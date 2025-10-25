mod config;
mod metrics;
mod persist;

use crate::config::AppConfig;
use crate::metrics::{extract_metrics_from_json, DetectionOutcome, ExtractedMetrics, MonitorState};
use crate::persist::{append_event_jsonl, load_state, save_state};
use anyhow::{Context, Result};
use chrono::Utc;
use clap::Parser;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Client;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;
use tokio::signal;

//simple CLI for toggling summary mode
#[derive(Debug, Parser)]
#[command(
    name = "bitaxe_monitor",
    version,
    about = "Polls device metrics and tracks bests"
)]
struct Cli {
    /// Print saved best metrics and exit
    #[arg(long)]
    summary: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    //choose config path from env or default
    let config_path = std::env::var("BITAXE_MONITOR_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config.json"));

    //initialize logging so runtime logs can be controlled via RUST_LOG
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    //parse CLI flags (e.g., --summary)
    let cli = Cli::parse();

    //load config file for user-defined endpoint and json pointers
    let config: AppConfig = config::load_config(&config_path)
        .with_context(|| format!("failed to load config at {:?}", config_path))?;

    //support a quick summary mode via --summary or bare "summary" arg
    let wants_summary = cli.summary
        || std::env::args()
            .skip(1)
            .any(|a| a == "summary" || a == "--summary");
    if wants_summary {
        if maybe_print_summary_and_exit(&config)? {
            return Ok(());
        }
    }

    //prepare http client with sensible timeouts
    let mut client_builder = Client::builder()
        .timeout(Duration::from_secs(config.http.timeout_secs.unwrap_or(10)))
        .user_agent("bitaxe-monitor/0.1");

    //parse headers once at startup so invalid names/values fail fast
    if let Some(hdrs) = &config.http.headers {
        let mut map = HeaderMap::new();
        for (k, v) in hdrs.iter() {
            let name = HeaderName::from_bytes(k.as_bytes())
                .with_context(|| format!("invalid header name: {}", k))?;
            let value = HeaderValue::from_str(v)
                .with_context(|| format!("invalid header value for {}: {}", k, v))?;
            map.append(name, value);
        }
        client_builder = client_builder.default_headers(map);
    }

    let client = client_builder
        .build()
        .context("failed to build http client")?;

    //preflight: validate pointers against a live response so failures surface fast
    preflight_check(&client, &config)
        .await
        .context("preflight failed: endpoint/pointers invalid or unreachable")?;

    //load prior state so we can keep all-time best across reboots
    let mut state = load_state(&config.storage.state_path).unwrap_or_else(|_| MonitorState::new());

    //write a startup event to help debugging timelines
    append_event_jsonl(
        &config.storage.events_path,
        serde_json::json!({
            "ts": Utc::now(),
            "event": "service_start",
            "config": {
                "endpoint_url": config.http.endpoint_url,
                "poll_interval_secs": config.poll_interval_secs
            }
        }),
    )?;

    //mask the endpoint url for security
    fn mask_endpoint(url: &str) -> String {
        if let Some(x) = url.find("://") {
            let (scheme, _rest) = url.split_at(x + 3); // keep "http://"
            let (_host, _tail) = _rest.split_once('/').unwrap_or((_rest, ""));// HIDE HOST/PATH so we dont need host or tail
            format!("{}[HIDDEN_ENDPOINT EVEN IF RUNNING LOCALLY]", scheme)
                } else {
                "[HOST-HIDDEN]".to_string()
            }
    }

    //print service start message WITH MASKED ENDPOINT URL FOR SECURITY
    println!("Starting [bitaxe_monitor] service: polling {} every {}s -> to exit, press Ctrl+C", mask_endpoint(&config.http.endpoint_url), config.poll_interval_secs);
    //do one poll immediately so first data shows up without waiting a full interval
    if let Err(err) = poll_once(&client, &config, &mut state).await {
        //log errors to events file so failures are visible later
        let _ = append_event_jsonl(
            &config.storage.events_path,
            serde_json::json!({
                "ts": Utc::now(),
                "event": "poll_error",
                "error": err.to_string()
            }),
        );
    }

    //run polling loop until ctrl+c
    let mut interval = tokio::time::interval(Duration::from_secs(config.poll_interval_secs));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(err) = poll_once(&client, &config, &mut state).await {
                    //log errors to events file so failures are visible later
                    let _ = append_event_jsonl(
                        &config.storage.events_path,
                        serde_json::json!({
                            "ts": Utc::now(),
                            "event": "poll_error",
                            "error": err.to_string()
                        })
                    );
                }
            }
            _ = signal::ctrl_c() => {
                let ts = Utc::now();
            let mut errs: Vec<String> = Vec::new();

            if let Err(err) = append_event_jsonl(
                &config.storage.events_path,
                serde_json::json!({"ts": ts, "event": "service_stop"}),
            ) {
                eprintln!("[bitaxe_monitor] WARN: failed to write service_stop: {err}");
                errs.push(format!("service_stop: {err}"));
            }

            if let Err(err) = save_state(&config.storage.state_path, &state) {
                eprintln!("[bitaxe_monitor] WARN: failed to save myBitAxeInfo.json: {err}");
                errs.push(format!("save_state: {err}"));
            }

            if errs.is_empty() {
                println!("[bitaxe_monitor] Graceful shutdown received → saved myBitAxeInfo.json and wrote service_stop to events.jsonl");
                break;
            } else {
                return Err(anyhow::anyhow!("shutdown errors: {}", errs.join("; ")));
            }
            }
        }
    }

    Ok(())
}
//check command-line args for a summary flag; if present, print best metrics and exit
fn maybe_print_summary_and_exit(config: &AppConfig) -> Result<bool> {
    //accept either "summary" or "--summary" for convenience
    let has_summary_flag = std::env::args()
        .skip(1)
        .any(|a| a == "summary" || a == "--summary");
    if !has_summary_flag {
        return Ok(false);
    }

    //load saved state so we can report best values observed so far
    match load_state(&config.storage.state_path) {
        Ok(state) => {
            println!("state file: {}", &config.storage.state_path);
            if let Some(v) = state.tool_best_hashrate_ths {
                println!("best hashrate: {:.2} TH/s", v);
            } else {
                println!("best hashrate: n/a");
            }
            if let Some(v) = state.tool_best_efficiency_j_per_th {
                println!("best efficiency: {:.2} J/TH", v);
            } else {
                println!("best efficiency: n/a");
            }
            if let Some(v) = state.last_displayed_all_time {
                println!("device all-time best: {:.2}", v);
            }
            if let Some(v) = state.last_displayed_boot_best {
                println!("device boot best: {:.2}", v);
            }
            println!(
                "monitor global best (internal): {:.2}",
                state.tool_global_all_time_best
            );
        }
        Err(_) => {
            println!("state file not found yet: {}", &config.storage.state_path);
            println!("run the monitor first to populate best values");
        }
    }
    Ok(true)
}

async fn poll_once(client: &Client, config: &AppConfig, state: &mut MonitorState) -> Result<()> {
    //fetch the endpoint json with simple retry/backoff so transient network errors do not cause missed polls
    let text = fetch_text_with_retries(client, config, 3, Duration::from_millis(500)).await?;
    let json: Value =
        serde_json::from_str(&text).with_context(|| "endpoint did not return valid json")?;

    //pull metric numbers from json using user-provided json pointers
    let ExtractedMetrics {
        displayed_all_time,
        displayed_boot_best,
        uptime_secs,
        boot_id,
        hashrate_ths,
        efficiency_j_per_th,
    } = extract_metrics_from_json(&json, &config.pointers)
        .with_context(|| "failed extracting metrics using json pointers")?;

    //evaluate for reboots and new bests
    let (eps_hash, eps_eff) = if let Some(t) = &config.thresholds {
        (
            t.epsilon_hashrate_ths.unwrap_or(0.01),
            t.epsilon_efficiency_j_per_th.unwrap_or(0.01),
        )
    } else {
        (0.01, 0.01)
    };
    let outcome = metrics::detect_changes(
        state,
        displayed_all_time,
        displayed_boot_best,
        uptime_secs,
        boot_id.as_deref(),
        hashrate_ths,
        efficiency_j_per_th,
        eps_hash,
        eps_eff,
    );

    //record events and persist state
    handle_detection_outcome(&config.storage.events_path, state, outcome)?;
    save_state(&config.storage.state_path, state)?;

    Ok(())
}

//make a few attempts with exponential backoff to get a response so short network glitches do not surface as errors
async fn fetch_text_with_retries(
    client: &Client,
    config: &AppConfig,
    max_retries: usize,
    base_delay: Duration,
) -> Result<String> {
    let mut attempt: usize = 0;
    loop {
        //rebuild request each attempt because RequestBuilder is single-use
        let req = client.get(&config.http.endpoint_url);

        let send_result = req.send().await;
        match send_result {
            Ok(resp) => match resp.error_for_status() {
                Ok(ok_resp) => match ok_resp.text().await {
                    Ok(body) => return Ok(body),
                    Err(err) => {
                        if attempt < max_retries {
                            let factor = 1u64 << attempt;
                            let delay_ms = (base_delay.as_millis() as u64).saturating_mul(factor);
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            attempt += 1;
                            continue;
                        } else {
                            return Err(anyhow::anyhow!(err));
                        }
                    }
                },
                Err(err) => {
                    if attempt < max_retries {
                        let factor = 1u64 << attempt;
                        let delay_ms = (base_delay.as_millis() as u64).saturating_mul(factor);
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        attempt += 1;
                        continue;
                    } else {
                        return Err(anyhow::anyhow!(err));
                    }
                }
            },
            Err(err) => {
                if attempt < max_retries {
                    let factor = 1u64 << attempt;
                    let delay_ms = (base_delay.as_millis() as u64).saturating_mul(factor);
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    attempt += 1;
                    continue;
                } else {
                    return Err(anyhow::anyhow!(err));
                }
            }
        }
    }
}

//fetch once and try extracting metrics so configuration problems are caught immediately
async fn preflight_check(client: &Client, config: &AppConfig) -> Result<()> {
    let text = fetch_text_with_retries(client, config, 2, Duration::from_millis(300)).await?;
    let json: Value = serde_json::from_str(&text)
        .with_context(|| "endpoint did not return valid json during preflight")?;
    let _ = extract_metrics_from_json(&json, &config.pointers).with_context(|| {
        "failed extracting metrics during preflight using configured json pointers"
    })?;
    Ok(())
}

fn handle_detection_outcome(
    path: &str,
    state: &MonitorState,
    outcome: DetectionOutcome,
) -> Result<()> {
    //write structured events based on detected changes so the events log shows reboots and new records in order
    let now = Utc::now();

    //record a boot event when a fresh start is observed so timelines show when the device restarted
    if outcome.boot_detected {
        append_event_jsonl(
            path,
            serde_json::json!({
                "ts": now,
                "event": "boot_detected",
                "state": state
            }),
        )?;
    }

    //record a session best when the current boot produces a new top value so each run keeps its own high-water mark
    if let Some(v) = outcome.new_device_boot_best {
        append_event_jsonl(
            path,
            serde_json::json!({
                "ts": now,
                "event": "new_device_boot_best",
                "value": v
            }),
        )?;
    }

    //record a lifetime best for this device when a new all-time high appears so progress across many runs is captured
    if let Some(v) = outcome.new_device_all_time_best {
        append_event_jsonl(
            path,
            serde_json::json!({
                "ts": now,
                "event": "new_device_all_time_best",
                "value": v
            }),
        )?;
    }

    //record the best value this tool has ever seen so the monitor can celebrate its own highest reading
    if let Some(v) = outcome.new_tool_all_time_best {
        append_event_jsonl(
            path,
            serde_json::json!({
                "ts": now,
                "event": "new_tool_all_time_best",
                "value": v
            }),
        )?;
    }

    // record new best hashrate (TH/s) when present
    if let Some(v) = outcome.new_tool_best_hashrate_ths {
        append_event_jsonl(
            path,
            serde_json::json!({
                "ts": now,
                "event": "new_tool_best_hashrate_ths",
                "value": v
            }),
        )?;
    }

    // record new best efficiency (lowest J/TH) when present
    if let Some(v) = outcome.new_tool_best_efficiency_j_per_th {
        append_event_jsonl(
            path,
            serde_json::json!({
                "ts": now,
                "event": "new_tool_best_efficiency_j_per_th",
                "value": v
            }),
        )?;
    }

    Ok(())
}
