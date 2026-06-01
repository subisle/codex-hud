use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{atomic::AtomicBool, mpsc::Sender, Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::hud::{HudSnapshot, RateLimitSummary};

pub(super) fn spawn(
    db_path: PathBuf,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    hud_dirty: Sender<()>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || collect_cc_switch_quota_state(db_path, snapshot, stop, hud_dirty))
}

pub(super) fn db_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_HUD_CC_SWITCH_DB") {
        return Some(path.into());
    }

    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cc-switch/cc-switch.db"))
}

fn collect_cc_switch_quota_state(
    db_path: PathBuf,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    hud_dirty: Sender<()>,
) {
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        if let Ok(Some(rate_limit)) = read_cc_switch_rate_limit(&db_path) {
            if let Ok(mut guard) = snapshot.lock() {
                if guard.rate_limit.as_ref() != Some(&rate_limit) {
                    guard.rate_limit = Some(rate_limit);
                    super::notify_hud_dirty(&hud_dirty);
                }
            }
        }

        if super::sleep_until_stop(&stop, 20, Duration::from_millis(500)) {
            return;
        }
    }
}

fn read_cc_switch_rate_limit(db_path: &Path) -> io::Result<Option<RateLimitSummary>> {
    if !db_path.exists() {
        return Ok(None);
    }

    let output = Command::new("sqlite3")
        .arg("-noheader")
        .arg("-separator")
        .arg("\t")
        .arg(db_path)
        .arg(CC_SWITCH_QUOTA_QUERY)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "sqlite3 exited with {status}",
            status = output.status
        )));
    }

    let row = String::from_utf8_lossy(&output.stdout);
    Ok(parse_cc_switch_quota_row(row.trim_end()))
}

const CC_SWITCH_QUOTA_QUERY: &str = r#"
WITH provider AS (
    SELECT
        name,
        CAST(NULLIF(limit_daily_usd, '') AS REAL) AS daily_limit,
        CAST(NULLIF(limit_monthly_usd, '') AS REAL) AS monthly_limit
    FROM providers
    WHERE app_type = 'codex'
    ORDER BY is_current DESC, sort_index, name
    LIMIT 1
),
selected AS (
    SELECT
        name,
        CASE
            WHEN daily_limit > 0 THEN 'daily'
            WHEN monthly_limit > 0 THEN 'monthly'
            ELSE 'none'
        END AS scope,
        CASE
            WHEN daily_limit > 0 THEN daily_limit
            WHEN monthly_limit > 0 THEN monthly_limit
            ELSE 0
        END AS limit_usd
    FROM provider
),
usage AS (
    SELECT
        CASE
            WHEN (SELECT scope FROM selected) = 'daily' THEN (
                SELECT COALESCE(SUM(CAST(total_cost_usd AS REAL)), 0)
                FROM proxy_request_logs
                WHERE app_type = 'codex'
                  AND date(created_at, 'unixepoch', 'localtime') = date('now', 'localtime')
            )
            WHEN (SELECT scope FROM selected) = 'monthly' THEN (
                SELECT COALESCE(SUM(CAST(total_cost_usd AS REAL)), 0)
                FROM proxy_request_logs
                WHERE app_type = 'codex'
                  AND strftime('%Y-%m', created_at, 'unixepoch', 'localtime') = strftime('%Y-%m', 'now', 'localtime')
            )
            ELSE 0
        END AS used_usd
)
SELECT
    selected.scope,
    printf('%.6f', usage.used_usd),
    printf('%.6f', selected.limit_usd),
    selected.name
FROM selected, usage;
"#;

fn parse_cc_switch_quota_row(row: &str) -> Option<RateLimitSummary> {
    if row.trim().is_empty() {
        return None;
    }

    let mut columns = row.split('\t');
    let scope = columns.next()?.trim();
    let used = parse_f64(columns.next()?)?;
    let limit = parse_f64(columns.next()?)?;
    let provider = columns
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let used_percent = if limit > 0.0 {
        ((used / limit) * 100.0).round().clamp(0.0, 100.0) as u8
    } else {
        0
    };
    let limit_label = match (scope, provider) {
        ("daily", Some(provider)) => Some(format!("cc-switch daily {provider}")),
        ("daily", None) => Some("cc-switch daily".to_string()),
        ("monthly", Some(provider)) => Some(format!("cc-switch monthly {provider}")),
        ("monthly", None) => Some("cc-switch monthly".to_string()),
        (_, Some(provider)) => Some(format!("cc-switch no limit {provider}")),
        _ => Some("cc-switch no limit".to_string()),
    };

    Some(RateLimitSummary {
        used_percent,
        limit_label,
    })
}

fn parse_f64(value: &str) -> Option<f64> {
    value.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cc_switch_quota_rows() {
        let rate_limit = parse_cc_switch_quota_row("daily\t12.500000\t50.000000\tSub2API").unwrap();
        assert_eq!(rate_limit.used_percent, 25);
        assert_eq!(
            rate_limit.limit_label.as_deref(),
            Some("cc-switch daily Sub2API")
        );
    }

    #[test]
    fn parses_cc_switch_rows_without_limits() {
        let rate_limit = parse_cc_switch_quota_row("none\t0.000000\t0.000000\tSub2API").unwrap();
        assert_eq!(rate_limit.used_percent, 0);
        assert_eq!(
            rate_limit.limit_label.as_deref(),
            Some("cc-switch no limit Sub2API")
        );
    }
}
