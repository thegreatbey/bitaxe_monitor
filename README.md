## bitaxe_monitor

A small async service that polls a device status endpoint, extracts metrics using JSON Pointers, detects reboots and new "best" values, appends JSONL events, and persists state between runs.

### Quick Start

**Bash**
```bash
cp config.example.json config.json
# edit config.json (endpoint_url and JSON pointers)
cargo run --release
```
### Summary mode
Print previously saved best metrics and exit (without polling):

```powershell
cargo run --release -- --summary
```

You can also pass a bare positional word:

```powershell
cargo run --release -- summary
```


### Requirements
- Rust toolchain (stable)

### Configure
- The app loads a config file path from the `BITAXE_MONITOR_CONFIG` environment variable; if not set, it defaults to `config.json` in the current directory:

```rust
let config_path = std::env::var("BITAXE_MONITOR_CONFIG")
    .map(PathBuf::from)
    .unwrap_or_else(|_| PathBuf::from("config.json"));
```

### Thresholds (optional)
- Defaults if omitted: `epsilon_hashrate_ths = 0.01`, `epsilon_efficiency_j_per_th = 0.01`.

```json
"thresholds": {
  "epsilon_hashrate_ths": 0.01,
  "epsilon_efficiency_j_per_th": 0.01
}
```

### Recommended setup
- Commit `config.example.json` with placeholders and keep real `config.json` local (add `config.json` to `.gitignore`).

### Two options to set config (PowerShell)
1) Copy the example when setting up on a new machine
```powershell
Copy-Item .\config.example.json .\config.json
```

2) Point to a private config path via env var
```powershell
$env:BITAXE_MONITOR_CONFIG="C:\\Users\\<you>\\config.json"
```

### Run
```powershell
cargo run --release
```

### Outputs
- `events.jsonl`: one JSON event per line (service start/stop, boot_detected, new bests, errors)
- `myBitAxeInfo.json`: persisted `MonitorState` to track device and tool-wide bests across runs

### Live view (tail) of events
- PowerShell (Windows):
```powershell
Get-Content .\events.jsonl -Wait
```
- Only show best metric updates:
```powershell
Get-Content .\events.jsonl -Wait | Select-String '"new_tool_best_'
```

### State file fields (`myBitAxeInfo.json`)
- `last_displayed_all_time`: device-reported all-time best (from `/bestDiff`)
- `last_displayed_boot_best`: device-reported current session best (from `/bestSessionDiff`)
- `last_uptime_secs`: last seen device uptime in seconds
- `last_boot_marker`: reboot marker (prefers boot_id; falls back to `"uptime:<n>"`)
- `tool_global_all_time_best`: highest device best the monitor has ever observed
- `tool_best_hashrate_ths`: highest hashrate (TH/s) observed (scaled if using GH/s)
- `tool_best_efficiency_j_per_th`: lowest J/TH observed (computed as `power_w / hashrate_ths` when not provided by device)

### Notes
- `pointers.json_pointer_boot_id` is optional; use `null` or remove the field if your endpoint lacks a boot ID. Both deserialize to no value.

- `events.jsonl` grows over time. For long-running deployments, consider rotating the file (e.g., copy and truncate on a schedule) or archiving old lines periodically.

