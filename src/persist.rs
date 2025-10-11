use crate::metrics::MonitorState;
use anyhow::Result;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Write};
use std::path::Path;

//add one JSON object per line to a file so event history stays simple to read and process later
pub fn append_event_jsonl(path: &str, value: impl Serialize) -> Result<()> {
    //create parent folder when path includes directories
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    //append one JSON object per line so large histories are easy to stream/process
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(&value)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    //for critical events (like service stop) call f.sync_all() to force write to disk
    f.flush()?;
    Ok(())
}

//write state safely using a temp file and a replace step so partial writes do not corrupt the saved state
pub fn save_state(path: &str, state: &MonitorState) -> Result<()> {
    let tmp = format!("{}.tmp", path);

    //create parent folder if missing
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    //write to a temp file first so a crash never leaves a half-written file
    {
        let mut f = File::create(&tmp)?;
        serde_json::to_writer_pretty(&mut f, state)?;
        f.flush()?;
        //fsync to persist to disk; reduces risk after power loss
        f.sync_all()?;
    }

    //try atomic rename first; on Windows fallback to delete then rename
    if let Err(e) = fs::rename(&tmp, path) {
        //Windows cannot overwrite with rename; remove destination then retry
        if Path::new(path).exists() {
            fs::remove_file(path)?;
            fs::rename(&tmp, path)?;
        } else {
            return Err(e.into());
        }
    }
    Ok(())
}

//read state from disk and turn JSON back into a MonitorState
pub fn load_state(path: &str) -> Result<MonitorState> {
    let f = File::open(path)?;
    let reader = BufReader::new(f);
    let state: MonitorState = serde_json::from_reader(reader)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_save_then_load_roundtrip() {
        // temp dir for isolated file IO keeps the workspace clean
        let dir = tempfile::tempdir().expect("tempdir");
        let state_path = dir.path().join("state.json");
        let state_path_str = state_path.to_string_lossy().to_string();

        // initial state with a few values to verify persistence
        let mut s = MonitorState::new();
        s.last_displayed_all_time = Some(10.0);
        s.last_displayed_boot_best = Some(7.5);
        s.last_uptime_secs = Some(120);
        s.tool_global_all_time_best = 10.0;

        save_state(&state_path_str, &s).expect("save_state");
        let loaded = load_state(&state_path_str).expect("load_state");
        assert_eq!(loaded.last_displayed_all_time, Some(10.0));
        assert_eq!(loaded.last_displayed_boot_best, Some(7.5));
        assert_eq!(loaded.last_uptime_secs, Some(120));
        assert!((loaded.tool_global_all_time_best - 10.0).abs() < 1e-9);

        // second save should overwrite via rename/replacement behavior
        let mut s2 = loaded.clone();
        s2.last_displayed_all_time = Some(11.0);
        save_state(&state_path_str, &s2).expect("save_state 2");
        let loaded2 = load_state(&state_path_str).expect("load_state 2");
        assert_eq!(loaded2.last_displayed_all_time, Some(11.0));

        // verify temp file not left behind after rename
        let tmp_exists = fs::metadata(format!("{}.tmp", state_path_str)).is_ok();
        assert!(!tmp_exists, "temp file should be removed after rename");
    }
}