use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

use crate::hud::HudSnapshot;

pub(super) fn spawn(
    real_codex: PathBuf,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    hud_dirty: Sender<()>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || collect_mcp_state(real_codex, snapshot, stop, hud_dirty))
}

fn collect_mcp_state(
    real_codex: PathBuf,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    hud_dirty: Sender<()>,
) {
    let mut last_count = None;

    while !stop.load(Ordering::Relaxed) {
        if let Ok(count) = read_enabled_mcp_count(&real_codex) {
            if last_count != Some(count) {
                if let Ok(mut guard) = snapshot.lock() {
                    if guard.mcp_count != count {
                        guard.mcp_count = count;
                        super::notify_hud_dirty(&hud_dirty);
                    }
                }
                last_count = Some(count);
            }
        }

        if super::sleep_until_stop(&stop, 20, Duration::from_millis(500)) {
            return;
        }
    }
}

fn read_enabled_mcp_count(real_codex: &Path) -> io::Result<u64> {
    let output = Command::new(real_codex)
        .args(["mcp", "list", "--json"])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "codex mcp list --json exited with {status}",
            status = output.status
        )));
    }

    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|err| io::Error::other(err.to_string()))?;
    Ok(count_enabled_mcp_servers(&value) as u64)
}

fn count_enabled_mcp_servers(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Array(items) => {
            items.iter().map(count_enabled_mcp_servers).sum::<usize>()
        }
        serde_json::Value::Object(object) => {
            if let Some(servers) = object.get("servers").and_then(serde_json::Value::as_array) {
                return servers.iter().map(count_enabled_mcp_servers).sum();
            }

            if let Some(enabled) = object.get("enabled").and_then(serde_json::Value::as_bool) {
                return usize::from(enabled);
            }

            if object.contains_key("transport")
                || object.contains_key("command")
                || object.contains_key("url")
            {
                return 1;
            }

            0
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_enabled_mcp_servers_from_json_payloads() {
        let value = serde_json::json!([
            { "name": "alpha", "enabled": true },
            { "name": "beta", "enabled": false },
            {
                "name": "gamma",
                "transport": { "type": "stdio" }
            }
        ]);

        assert_eq!(count_enabled_mcp_servers(&value), 2);
    }
}
