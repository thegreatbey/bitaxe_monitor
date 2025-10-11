use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    pub endpoint_url: String,
    pub headers: Option<HashMap<String, String>>, //use for auth tokens if needed
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonPointers {
    pub json_pointer_all_time: String,
    pub json_pointer_boot_best: String,
    pub json_pointer_uptime_secs: Option<String>,
    pub json_pointer_boot_id: Option<String>,
    // pointers for current hashrate (TH/s) and efficiency (J/TH); optional to avoid breaking older configs
    pub json_pointer_hashrate_ths: Option<String>,
    pub json_pointer_efficiency_j_per_th: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub events_path: String,
    pub state_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub http: HttpConfig,
    pub pointers: JsonPointers,
    pub poll_interval_secs: u64,
    pub storage: StorageConfig,
    // optional tuning to reduce jitter when updating bests
    pub thresholds: Option<ThresholdsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdsConfig {
    pub epsilon_hashrate_ths: Option<f64>,
    pub epsilon_efficiency_j_per_th: Option<f64>,
}

pub fn load_config<P: AsRef<Path>>(path: P) -> Result<AppConfig> {
    let bytes = fs::read(path.as_ref())
        .with_context(|| "failed to read config file")?;
    //support both json and toml by sniffing the first non-space char
    let text = String::from_utf8(bytes).context("config is not utf-8")?;
    let first = text.chars().find(|c| !c.is_whitespace());
    let cfg: AppConfig = match first {
        Some('{') => serde_json::from_str(&text).context("invalid json config")?,
        _ => toml::from_str(&text).context("invalid toml config")?,
    };

    validate_config(&cfg)?;
    Ok(cfg)
}

fn validate_config(cfg: &AppConfig) -> Result<()> {
    if cfg.poll_interval_secs == 0 {
        bail!("poll_interval_secs must be > 0");
    }
    if !cfg.http.endpoint_url.starts_with("http://") && !cfg.http.endpoint_url.starts_with("https://") {
        bail!("endpoint_url must start with http:// or https://");
    }

    //require json pointers to begin with '/' so pointer semantics match serde_json::Value::pointer
    let ptrs = &cfg.pointers;
    let mut bad: Vec<(&str, &str)> = Vec::new();
    if !ptrs.json_pointer_all_time.starts_with('/') { bad.push(("json_pointer_all_time", &ptrs.json_pointer_all_time)); }
    if !ptrs.json_pointer_boot_best.starts_with('/') { bad.push(("json_pointer_boot_best", &ptrs.json_pointer_boot_best)); }
    if let Some(p) = &ptrs.json_pointer_uptime_secs { if !p.starts_with('/') { bad.push(("json_pointer_uptime_secs", p)); } }
    if let Some(p) = &ptrs.json_pointer_boot_id { if !p.starts_with('/') { bad.push(("json_pointer_boot_id", p)); } }
    if let Some(p) = &ptrs.json_pointer_hashrate_ths { if !p.starts_with('/') { bad.push(("json_pointer_hashrate_ths", p)); } }
    if let Some(p) = &ptrs.json_pointer_efficiency_j_per_th { if !p.starts_with('/') { bad.push(("json_pointer_efficiency_j_per_th", p)); } }
    if !bad.is_empty() {
        let joined = bad.into_iter().map(|(k,v)| format!("{}='{}'", k, v)).collect::<Vec<_>>().join(", ");
        bail!("json pointers must start with '/': {}", joined);
    }

    //validate thresholds when provided so negative or non-finite values are rejected early
    if let Some(t) = &cfg.thresholds {
        if let Some(v) = t.epsilon_hashrate_ths { if !(v.is_finite() && v >= 0.0) { bail!("epsilon_hashrate_ths must be >= 0 and finite"); } }
        if let Some(v) = t.epsilon_efficiency_j_per_th { if !(v.is_finite() && v >= 0.0) { bail!("epsilon_efficiency_j_per_th must be >= 0 and finite"); } }
    }
    Ok(())
}


