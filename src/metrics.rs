use crate::config::JsonPointers;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MonitorState {
    pub last_displayed_all_time: Option<f64>,
    pub last_displayed_boot_best: Option<f64>,
    pub last_uptime_secs: Option<u64>,
    pub last_boot_marker: Option<String>,
    pub tool_global_all_time_best: f64,
    // track tool-best hashrate (max TH/s) and efficiency (min J/TH)
    pub tool_best_hashrate_ths: Option<f64>,
    pub tool_best_efficiency_j_per_th: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _note: Option<String>,
}

impl MonitorState {
    pub fn new() -> Self {
        Self {
            last_displayed_all_time: None,
            last_displayed_boot_best: None,
            last_uptime_secs: None,
            last_boot_marker: None,
            tool_global_all_time_best: 0.0,
            tool_best_hashrate_ths: None,
            tool_best_efficiency_j_per_th: None,
            _note: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExtractedMetrics {
    pub displayed_all_time: f64,
    pub displayed_boot_best: f64,
    pub uptime_secs: Option<u64>,
    pub boot_id: Option<String>,
    // optional live metrics for hashrate (TH/s) and efficiency (J/TH)
    pub hashrate_ths: Option<f64>,
    pub efficiency_j_per_th: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct DetectionOutcome {
    pub boot_detected: bool,
    pub new_device_all_time_best: Option<f64>,
    pub new_device_boot_best: Option<f64>,
    pub new_tool_all_time_best: Option<f64>,
    // records when monitor observes new maxima/minima for live stats
    pub new_tool_best_hashrate_ths: Option<f64>,
    pub new_tool_best_efficiency_j_per_th: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub uptime_secs: Option<u64>,
    pub boot_id: Option<String>,
    pub hashrate_ths: Option<f64>,
    pub efficiency_j_per_th: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Thresholds {
    pub epsilon_hashrate_ths: f64,
    pub epsilon_efficiency_j_per_th: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Displayed {
    pub all_time: f64,
    pub boot_best: f64,
}

pub fn extract_metrics_from_json(
    json: &Value,
    ptrs: &JsonPointers,
) -> anyhow::Result<ExtractedMetrics> {
    //convert json pointer value to f64 to support numeric strings
    fn extract_f64(json: &Value, pointer: &str) -> anyhow::Result<f64> {
        let v = json
            .pointer(pointer)
            .ok_or_else(|| anyhow::anyhow!(format!("json pointer not found: {}", pointer)))?;
        match v {
            Value::Number(n) => n
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("number out of range")),
            Value::String(s) => parse_number_with_unit(s)
                .map_err(|e| anyhow::anyhow!(format!("{} at {}", e, pointer))),
            _ => Err(anyhow::anyhow!(format!("non-numeric value at {}", pointer))),
        }
    }

    fn extract_u64_opt(json: &Value, pointer_opt: &Option<String>) -> anyhow::Result<Option<u64>> {
        if let Some(pointer) = pointer_opt.as_ref() {
            let v = json
                .pointer(pointer)
                .ok_or_else(|| anyhow::anyhow!(format!("json pointer not found: {}", pointer)))?;
            match v {
                Value::Number(n) => n
                    .as_u64()
                    .map(Some)
                    .ok_or_else(|| anyhow::anyhow!("number out of range")),
                Value::String(s) => Ok(Some(
                    s.parse::<u64>()
                        .map_err(|_| anyhow::anyhow!("invalid integer"))?,
                )),
                _ => Err(anyhow::anyhow!("non-integer uptime")),
            }
        } else {
            Ok(None)
        }
    }

    fn extract_string_opt(
        json: &Value,
        pointer_opt: &Option<String>,
    ) -> anyhow::Result<Option<String>> {
        if let Some(pointer) = pointer_opt.as_ref() {
            let v = json
                .pointer(pointer)
                .ok_or_else(|| anyhow::anyhow!(format!("json pointer not found: {}", pointer)))?;
            match v {
                Value::String(s) => Ok(Some(s.clone())),
                Value::Number(n) => Ok(Some(n.to_string())),
                _ => Err(anyhow::anyhow!("boot id must be string or number")),
            }
        } else {
            Ok(None)
        }
    }

    let displayed_all_time = extract_f64(json, &ptrs.json_pointer_all_time)?;
    let displayed_boot_best = extract_f64(json, &ptrs.json_pointer_boot_best)?;

    //reject NaN/inf so downstream logic only sees real numbers
    if !displayed_all_time.is_finite() {
        return Err(anyhow::anyhow!("non-finite all_time value"));
    }
    if !displayed_boot_best.is_finite() {
        return Err(anyhow::anyhow!("non-finite boot_best value"));
    }
    let uptime_secs = extract_u64_opt(json, &ptrs.json_pointer_uptime_secs)?;
    let boot_id = extract_string_opt(json, &ptrs.json_pointer_boot_id)?;

    // helper to extract optional f64 given an optional pointer
    fn extract_f64_opt(json: &Value, pointer_opt: &Option<String>) -> anyhow::Result<Option<f64>> {
        if let Some(p) = pointer_opt.as_ref() {
            Ok(Some(extract_f64(json, p)?))
        } else {
            Ok(None)
        }
    }

    // optional: extract hashrate and apply scale to TH/s when configured (e.g., GH/s -> TH/s)
    let mut hashrate_ths = extract_f64_opt(json, &ptrs.json_pointer_hashrate_ths)?;
    if let (Some(scale), Some(h)) = (ptrs.hashrate_scale, hashrate_ths) {
        hashrate_ths = Some(h * scale);
    }

    // optional: extract efficiency directly when provided
    let mut efficiency_j_per_th = extract_f64_opt(json, &ptrs.json_pointer_efficiency_j_per_th)?;

    // optional: extract power (W) and compute efficiency when not provided
    if efficiency_j_per_th.is_none() {
        if let (Some(power_w), Some(h_ths)) = (
            extract_f64_opt(json, &ptrs.json_pointer_power_w)?,
            hashrate_ths,
        ) {
            if power_w.is_finite() && h_ths.is_finite() && h_ths > 0.0 {
                efficiency_j_per_th = Some(power_w / h_ths);
            }
        }
    }

    Ok(ExtractedMetrics {
        displayed_all_time,
        displayed_boot_best,
        uptime_secs,
        boot_id,
        hashrate_ths,
        efficiency_j_per_th,
    })
}

pub fn detect_changes(
    state: &mut MonitorState,
    displayed: Displayed,
    metrics: Metrics,
    thresholds: Thresholds,
) -> DetectionOutcome {
    let mut out = DetectionOutcome::default();

    let displayed_all_time = displayed.all_time;
    let displayed_boot_best = displayed.boot_best;
    let uptime_secs = metrics.uptime_secs;
    let boot_id_str = metrics.boot_id.as_deref();

    //derive boot marker priority: boot_id > uptime_secs > boot_best reset heuristic
    //used combinators map and or_else to simplify the code
    let current_marker = boot_id_str
        .map(|id| id.to_string())
        .or_else(|| uptime_secs.map(|up| format!("uptime:{}", up)));

    //detect reboot only on real signals so we do not emit on every poll
    // prefer explicit boot_id changes; otherwise detect when uptime decreases; finally, fall back to device boot-best reset
    let boot_id_changed = match (state.last_boot_marker.as_ref(), boot_id_str) {
        // compare only when the previous marker was also a boot_id (not an uptime-derived marker)
        (Some(prev_marker), Some(curr_id)) if !prev_marker.starts_with("uptime:") => {
            prev_marker != curr_id
        }
        _ => false,
    };

    if boot_id_changed {
        out.boot_detected = true;
    } else if let (Some(prev_up), Some(curr_up)) = (state.last_uptime_secs, uptime_secs) {
        if curr_up < prev_up {
            out.boot_detected = true;
        }
    } else if let Some(prev_boot_best) = state.last_displayed_boot_best {
        if displayed_boot_best + f64::EPSILON < prev_boot_best {
            out.boot_detected = true;
        }
    }

    //update state boot marker info
    state.last_boot_marker = current_marker;
    state.last_uptime_secs = uptime_secs;

    //detect device-reported new bests
    if let Some(prev) = state.last_displayed_boot_best {
        if displayed_boot_best > prev {
            out.new_device_boot_best = Some(displayed_boot_best);
        }
    } else {
        out.new_device_boot_best = Some(displayed_boot_best);
    }

    if let Some(prev) = state.last_displayed_all_time {
        if displayed_all_time > prev {
            out.new_device_all_time_best = Some(displayed_all_time);
        }
    } else {
        out.new_device_all_time_best = Some(displayed_all_time);
    }

    //update state for device values
    state.last_displayed_boot_best = Some(displayed_boot_best);
    state.last_displayed_all_time = Some(displayed_all_time);

    //track tool-global all-time best regardless of device resets
    let candidate = displayed_all_time.max(displayed_boot_best);
    if candidate > state.tool_global_all_time_best {
        state.tool_global_all_time_best = candidate;
        out.new_tool_all_time_best = Some(candidate);
    }

    // track best hashrate (max). only compare when value present and finite
    if let Some(h) = metrics.hashrate_ths.filter(|v| v.is_finite()) {
        //require small improvement to avoid jitter updates
        let is_better = match state.tool_best_hashrate_ths {
            Some(prev) => h - prev >= thresholds.epsilon_hashrate_ths,
            None => true,
        };
        if is_better {
            state.tool_best_hashrate_ths = Some(h);
            out.new_tool_best_hashrate_ths = Some(h);
        }
    }

    // track best efficiency (min J/TH). only compare when value present and finite
    if let Some(eff) = metrics.efficiency_j_per_th.filter(|v| v.is_finite()) {
        //require small decrease to avoid jitter updates
        let is_better = match state.tool_best_efficiency_j_per_th {
            Some(prev) => prev - eff >= thresholds.epsilon_efficiency_j_per_th,
            None => true,
        };
        if is_better {
            state.tool_best_efficiency_j_per_th = Some(eff);
            out.new_tool_best_efficiency_j_per_th = Some(eff);
        }
    }

    out
}

//parses numbers that may have unit suffixes like 1.22G or 22.6M
//supports K (1e3), M (1e6), G (1e9), T (1e12); falls back to plain float
fn parse_number_with_unit(input: &str) -> anyhow::Result<f64> {
    let s = input.trim();
    if s.is_empty() {
        return Err(anyhow::anyhow!("empty string"));
    }

    // take the first whitespace-separated token so strings like "16.09 J/TH" parse as 16.09
    let token = s.split_whitespace().next().unwrap();

    // try plain float first so values like "NaN" or "inf" are handled by Rust's f64 parser
    // this allows a later is_finite() check to produce a clear "non-finite" error
    if let Ok(v) = token.parse::<f64>() {
        return Ok(v);
    }

    let last = token.chars().last().unwrap();
    if last.is_ascii_alphabetic() {
        let unit_char = last.to_ascii_uppercase();
        let number_part = &token[..token.len() - 1];
        let base: f64 = number_part.trim().parse()?;
        let factor = match unit_char {
            'K' => 1e3,
            'M' => 1e6,
            'G' => 1e9,
            'T' => 1e12,
            _ => return Err(anyhow::anyhow!("unsupported unit suffix")),
        };
        Ok(base * factor)
    } else {
        Ok(token.parse()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::JsonPointers;

    #[test]
    fn test_parse_number_with_unit_plain() {
        let v = super::parse_number_with_unit("123.5").unwrap();
        assert!((v - 123.5).abs() < 1e-9);
    }

    #[test]
    fn test_parse_number_with_unit_units() {
        assert!((super::parse_number_with_unit("1K").unwrap() - 1e3).abs() < 1e-6);
        assert!((super::parse_number_with_unit("1.2M").unwrap() - 1.2e6).abs() < 1e-3);
        assert!((super::parse_number_with_unit("0.5G").unwrap() - 0.5e9).abs() < 1.0);
    }

    #[test]
    fn test_detect_changes_boot_and_bests() {
        let mut state = MonitorState::new();
        // initial values should set new bests
        let displayed = Displayed {
            all_time: 10.0,
            boot_best: 5.0,
        };
        let metrics = Metrics {
            uptime_secs: Some(100),
            boot_id: None,
            hashrate_ths: None,
            efficiency_j_per_th: None,
        };
        let thresholds = Thresholds {
            epsilon_hashrate_ths: 0.01,
            epsilon_efficiency_j_per_th: 0.01,
        };
        let out1 = detect_changes(&mut state, displayed, metrics, thresholds);
        assert!(out1.new_device_all_time_best.is_some());
        assert!(out1.new_device_boot_best.is_some());
        assert!(out1.new_tool_all_time_best.is_some());

        // higher boot best updates boot best and tool best
        let displayed = Displayed {
            all_time: 10.0,
            boot_best: 6.0,
        };
        let metrics = Metrics {
            uptime_secs: Some(110),
            boot_id: None,
            hashrate_ths: None,
            efficiency_j_per_th: None,
        };
        let thresholds = Thresholds {
            epsilon_hashrate_ths: 0.01,
            epsilon_efficiency_j_per_th: 0.01,
        };
        let out2 = detect_changes(&mut state, displayed, metrics, thresholds);
        assert!(out2.new_device_boot_best.is_some());

        // simulate reboot via uptime drop
        let displayed = Displayed {
            all_time: 9.0,
            boot_best: 4.0,
        };
        let metrics = Metrics {
            uptime_secs: Some(10),
            boot_id: None,
            hashrate_ths: None,
            efficiency_j_per_th: None,
        };
        let thresholds = Thresholds {
            epsilon_hashrate_ths: 0.01,
            epsilon_efficiency_j_per_th: 0.01,
        };
        let out3 = detect_changes(&mut state, displayed, metrics, thresholds);
        assert!(out3.boot_detected);
    }

    #[test]
    fn test_parse_number_with_unit_invalid_suffix() {
        let err = super::parse_number_with_unit("1.2X").unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.to_lowercase().contains("unsupported"));
    }

    #[test]
    fn test_detect_changes_boot_id_priority() {
        let mut state = MonitorState::new();
        // initial with boot_id "A"
        let displayed = Displayed {
            all_time: 1.0,
            boot_best: 1.0,
        };
        let metrics = Metrics {
            uptime_secs: Some(50),
            boot_id: Some("A".to_string()),
            hashrate_ths: None,
            efficiency_j_per_th: None,
        };
        let thresholds = Thresholds {
            epsilon_hashrate_ths: 0.01,
            epsilon_efficiency_j_per_th: 0.01,
        };
        let _ = detect_changes(&mut state, displayed, metrics, thresholds);
        // change only boot_id to "B" (uptime increases), expect boot_detected
        let displayed = Displayed {
            all_time: 1.1,
            boot_best: 1.1,
        };
        let metrics = Metrics {
            uptime_secs: Some(60),
            boot_id: Some("B".to_string()),
            hashrate_ths: None,
            efficiency_j_per_th: None,
        };
        let thresholds = Thresholds {
            epsilon_hashrate_ths: 0.01,
            epsilon_efficiency_j_per_th: 0.01,
        };
        let out = detect_changes(&mut state, displayed, metrics, thresholds);
        assert!(out.boot_detected);
    }

    #[test]
    fn test_extract_metrics_numbers() {
        // numbers as native json numbers keep parsing simple and precise
        let json = serde_json::json!({
            "all_time": 12.5,
            "boot_best": 8.75,
            "uptime": 1234,
            "boot_id": "XYZ",
            "hashrate": 1.5,
            "efficiency": 16.1
        });

        let ptrs = JsonPointers {
            json_pointer_all_time: "/all_time".into(),
            json_pointer_boot_best: "/boot_best".into(),
            json_pointer_uptime_secs: Some("/uptime".into()),
            json_pointer_boot_id: Some("/boot_id".into()),
            json_pointer_hashrate_ths: Some("/hashrate".into()),
            json_pointer_efficiency_j_per_th: Some("/efficiency".into()),
            json_pointer_power_w: None,
            hashrate_scale: None,
        };

        let m = extract_metrics_from_json(&json, &ptrs).unwrap();
        assert!((m.displayed_all_time - 12.5).abs() < 1e-9);
        assert!((m.displayed_boot_best - 8.75).abs() < 1e-9);
        assert_eq!(m.uptime_secs, Some(1234));
        assert_eq!(m.boot_id.as_deref(), Some("XYZ"));
        assert_eq!(m.hashrate_ths, Some(1.5));
        assert_eq!(m.efficiency_j_per_th, Some(16.1));
    }

    #[test]
    fn test_extract_metrics_string_units() {
        // strings with units still parse into floats so configs can point at display fields
        let json = serde_json::json!({
            "all_time": "10.0",
            "boot_best": "8.0",
            "uptime": "200",
            "boot_id": 42,
            "hashrate": "1.2T",
            "efficiency": "16.09 J/TH"
        });

        let ptrs = JsonPointers {
            json_pointer_all_time: "/all_time".into(),
            json_pointer_boot_best: "/boot_best".into(),
            json_pointer_uptime_secs: Some("/uptime".into()),
            json_pointer_boot_id: Some("/boot_id".into()),
            json_pointer_hashrate_ths: Some("/hashrate".into()),
            json_pointer_efficiency_j_per_th: Some("/efficiency".into()),
            json_pointer_power_w: None,
            hashrate_scale: None,
        };

        let m = extract_metrics_from_json(&json, &ptrs).unwrap();
        assert!((m.displayed_all_time - 10.0).abs() < 1e-9);
        assert!((m.displayed_boot_best - 8.0).abs() < 1e-9);
        assert_eq!(m.uptime_secs, Some(200));
        assert_eq!(m.boot_id.as_deref(), Some("42"));
        // 1.2T => 1.2e12
        assert!((m.hashrate_ths.unwrap() - 1.2e12).abs() < 1.0);
        // 16.09 J/TH => 16.09
        assert!((m.efficiency_j_per_th.unwrap() - 16.09).abs() < 1e-6);
    }

    #[test]
    fn test_extract_metrics_missing_pointer_errors() {
        // missing pointers should surface as errors to catch config mistakes fast
        let json = serde_json::json!({ "boot_best": 5 });
        let ptrs = JsonPointers {
            json_pointer_all_time: "/missing".into(),
            json_pointer_boot_best: "/boot_best".into(),
            json_pointer_uptime_secs: None,
            json_pointer_boot_id: None,
            json_pointer_hashrate_ths: None,
            json_pointer_efficiency_j_per_th: None,
            json_pointer_power_w: None,
            hashrate_scale: None,
        };
        let err = extract_metrics_from_json(&json, &ptrs).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.to_lowercase().contains("pointer"));
    }

    #[test]
    fn test_extract_metrics_non_finite_rejected() {
        // NaN should be rejected so downstream logic avoids invalid math
        let json = serde_json::json!({ "all_time": "NaN", "boot_best": 1 });
        let ptrs = JsonPointers {
            json_pointer_all_time: "/all_time".into(),
            json_pointer_boot_best: "/boot_best".into(),
            json_pointer_uptime_secs: None,
            json_pointer_boot_id: None,
            json_pointer_hashrate_ths: None,
            json_pointer_efficiency_j_per_th: None,
            json_pointer_power_w: None,
            hashrate_scale: None,
        };
        let err = extract_metrics_from_json(&json, &ptrs).unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.to_lowercase().contains("non-finite"));
    }
}
