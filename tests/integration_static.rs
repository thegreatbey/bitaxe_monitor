use bitaxe_monitor::config::JsonPointers;
use bitaxe_monitor::metrics::{detect_changes, extract_metrics_from_json, MonitorState};

#[test]
fn extractor_and_state_flow_over_static_json() {
    // static json simulates one device response
    let json = serde_json::json!({
        "all_time": 12.0,
        "boot_best": 9.0,
        "uptime": 300,
        "boot_id": "B1",
        "hashrate": 1.6,
        "efficiency": 15.8
    });

    // config pointers point to the fields above
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

    // extract metrics
    let m = extract_metrics_from_json(&json, &ptrs).expect("extract metrics");
    assert_eq!(m.uptime_secs, Some(300));
    assert_eq!(m.boot_id.as_deref(), Some("B1"));

    // run through state change detection once
    let mut state = MonitorState::new();
    let out = detect_changes(
        &mut state,
        m.displayed_all_time,
        m.displayed_boot_best,
        m.uptime_secs,
        m.boot_id.as_deref(),
        m.hashrate_ths,
        m.efficiency_j_per_th,
        0.01,
        0.01,
    );

    // first run should set initial bests
    assert!(out.new_device_all_time_best.is_some());
    assert!(out.new_device_boot_best.is_some());
}
