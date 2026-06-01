use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{atomic::AtomicBool, mpsc::Sender, Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::config::QuotaConfig;
use crate::hud::{HudSnapshot, RateLimitSummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum QuotaSource {
    CcSwitchDb(PathBuf),
    UsageScript(CcSwitchUsageScript),
}

impl QuotaSource {
    pub(super) fn from_config(config: &QuotaConfig) -> Option<Self> {
        if config.enabled {
            let api_key = std::env::var(&config.api_key_env).ok()?;
            let usage_url = config.usage_url.trim().trim_end_matches('/');
            if !usage_url.is_empty() && !api_key.trim().is_empty() {
                return Some(Self::UsageScript(CcSwitchUsageScript {
                    base_url: usage_url.to_string(),
                    api_key: api_key.trim().to_string(),
                    timeout_secs: config.timeout_secs.clamp(1, 60),
                }));
            }
        }

        db_path().map(Self::CcSwitchDb)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CcSwitchProvider {
    name: String,
    usage_script: Option<CcSwitchUsageScript>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CcSwitchUsageScript {
    base_url: String,
    api_key: String,
    timeout_secs: u64,
}

pub(super) fn spawn(
    source: QuotaSource,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    hud_dirty: Sender<()>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || collect_cc_switch_quota_state(source, snapshot, stop, hud_dirty))
}

pub(super) fn db_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CODEX_HUD_CC_SWITCH_DB") {
        return Some(path.into());
    }

    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cc-switch/cc-switch.db"))
}

fn collect_cc_switch_quota_state(
    source: QuotaSource,
    snapshot: Arc<Mutex<HudSnapshot>>,
    stop: Arc<AtomicBool>,
    hud_dirty: Sender<()>,
) {
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        if let Ok(Some(rate_limit)) = read_quota_source(&source) {
            if let Ok(mut guard) = snapshot.lock() {
                if guard.merge_rate_limit(rate_limit) {
                    super::notify_hud_dirty(&hud_dirty);
                }
            }
        }

        if super::sleep_until_stop(&stop, 20, Duration::from_millis(500)) {
            return;
        }
    }
}

fn read_quota_source(source: &QuotaSource) -> io::Result<Option<RateLimitSummary>> {
    match source {
        QuotaSource::CcSwitchDb(db_path) => read_cc_switch_rate_limit(db_path),
        QuotaSource::UsageScript(script) => fetch_cc_switch_usage(script, None),
    }
}

fn read_cc_switch_rate_limit(db_path: &Path) -> io::Result<Option<RateLimitSummary>> {
    if !db_path.exists() {
        return Ok(None);
    }

    let provider = read_cc_switch_provider(db_path).ok().flatten();
    select_cc_switch_rate_limit(provider.as_ref(), fetch_cc_switch_usage, || {
        read_cc_switch_legacy_rate_limit(db_path)
    })
}

fn read_cc_switch_provider(db_path: &Path) -> io::Result<Option<CcSwitchProvider>> {
    let output = Command::new("sqlite3")
        .arg("-noheader")
        .arg("-separator")
        .arg("\t")
        .arg(db_path)
        .arg(CC_SWITCH_PROVIDER_QUERY)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "sqlite3 exited with {status}",
            status = output.status
        )));
    }

    let row = String::from_utf8_lossy(&output.stdout);
    Ok(parse_cc_switch_provider_row(row.trim_end()))
}

fn read_cc_switch_legacy_rate_limit(db_path: &Path) -> io::Result<Option<RateLimitSummary>> {
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

const CC_SWITCH_PROVIDER_QUERY: &str = r#"
SELECT
    name,
    COALESCE(meta, '') AS meta
FROM providers
WHERE app_type = 'codex'
ORDER BY is_current DESC, sort_index, name
LIMIT 1;
"#;

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

fn parse_cc_switch_provider_row(row: &str) -> Option<CcSwitchProvider> {
    if row.trim().is_empty() {
        return None;
    }

    let mut columns = row.splitn(2, '\t');
    let name = columns.next()?.trim().to_string();
    let meta = columns.next().map(str::trim).unwrap_or_default();

    Some(CcSwitchProvider {
        name,
        usage_script: parse_usage_script(meta),
    })
}

fn parse_usage_script(meta: &str) -> Option<CcSwitchUsageScript> {
    let meta: Value = serde_json::from_str(meta).ok()?;
    let script = meta
        .get("usage_script")
        .or_else(|| meta.get("usageScript"))?;
    let script = match script {
        Value::String(text) => serde_json::from_str::<Value>(text).ok()?,
        Value::Object(_) => script.clone(),
        _ => return None,
    };
    let object = script.as_object()?;
    if !object
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }

    let base_url = object
        .get("baseUrl")
        .or_else(|| object.get("base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .trim_end_matches('/')
        .to_string();
    let api_key = object
        .get("apiKey")
        .or_else(|| object.get("api_key"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let timeout_secs = object
        .get("timeout")
        .and_then(json_u64)
        .unwrap_or(10)
        .clamp(1, 60);

    Some(CcSwitchUsageScript {
        base_url,
        api_key,
        timeout_secs,
    })
}

fn select_cc_switch_rate_limit(
    provider: Option<&CcSwitchProvider>,
    fetch_usage: impl FnOnce(&CcSwitchUsageScript, Option<&str>) -> io::Result<Option<RateLimitSummary>>,
    read_legacy: impl FnOnce() -> io::Result<Option<RateLimitSummary>>,
) -> io::Result<Option<RateLimitSummary>> {
    if let Some(provider) = provider {
        if let Some(script) = provider.usage_script.as_ref() {
            if let Ok(Some(rate_limit)) = fetch_usage(script, Some(provider.name.as_str())) {
                return Ok(Some(rate_limit));
            }
        }
    }

    read_legacy()
}

fn fetch_cc_switch_usage(
    script: &CcSwitchUsageScript,
    provider: Option<&str>,
) -> io::Result<Option<RateLimitSummary>> {
    let url = format!("{}/v1/usage", script.base_url);
    let output = Command::new("curl")
        .arg("-fsS")
        .arg("--max-time")
        .arg(script.timeout_secs.to_string())
        .arg("-H")
        .arg(format!("Authorization: Bearer {}", script.api_key))
        .arg(url)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "cc-switch usage request exited with {status}",
            status = output.status
        )));
    }

    let value: Value =
        serde_json::from_slice(&output.stdout).map_err(|err| io::Error::other(err.to_string()))?;
    Ok(parse_cc_switch_usage_response(&value, provider))
}

fn parse_cc_switch_usage_response(
    value: &Value,
    provider: Option<&str>,
) -> Option<RateLimitSummary> {
    let object = value.as_object()?;
    let quota = object.get("quota").and_then(Value::as_object);
    let usage = object.get("usage").and_then(Value::as_object);
    let today = usage.and_then(|usage| usage.get("today").and_then(Value::as_object));

    let limit_usd = quota
        .and_then(|quota| quota.get("limit"))
        .and_then(json_f64)
        .or_else(|| object.get("limit").and_then(json_f64));
    let quota_used = quota
        .and_then(|quota| quota.get("used"))
        .and_then(json_f64)
        .or_else(|| object.get("used").and_then(json_f64));
    let remaining_usd = quota
        .and_then(|quota| quota.get("remaining"))
        .and_then(json_f64)
        .or_else(|| object.get("remaining").and_then(json_f64))
        .or_else(|| object.get("balance").and_then(json_f64));
    let derived_used = limit_usd
        .zip(remaining_usd)
        .map(|(limit, remaining)| (limit - remaining).max(0.0));
    let usage_cost = today
        .and_then(|today| today.get("actual_cost"))
        .and_then(json_f64)
        .or_else(|| today.and_then(|today| today.get("cost")).and_then(json_f64))
        .or_else(|| object.get("cost").and_then(json_f64));
    let cost_usd = quota_used.or(derived_used).or(usage_cost);
    let remaining_usd = remaining_usd.or_else(|| {
        limit_usd
            .zip(cost_usd)
            .map(|(limit, cost)| (limit - cost).max(0.0))
    });
    let used_for_percent = quota_used.or(derived_used).or(cost_usd);
    let used_percent = match (used_for_percent, limit_usd) {
        (Some(used), Some(limit)) if limit > 0.0 => {
            ((used / limit) * 100.0).round().clamp(0.0, 100.0) as u8
        }
        _ => 0,
    };
    let label = provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .map(|provider| format!("cc-switch usage {provider}"))
        .unwrap_or_else(|| "cc-switch usage".to_string());

    Some(RateLimitSummary {
        used_percent,
        cost_usd,
        remaining_usd,
        limit_usd,
        limit_label: Some(label),
    })
}

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
    let has_limit = limit > 0.0;

    Some(RateLimitSummary {
        used_percent,
        cost_usd: (has_limit || used > 0.0).then_some(used),
        remaining_usd: has_limit.then_some((limit - used).max(0.0)),
        limit_usd: has_limit.then_some(limit),
        limit_label,
    })
}

fn parse_f64(value: &str) -> Option<f64> {
    let value = value.trim().parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

fn json_f64(value: &Value) -> Option<f64> {
    let value = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }?;
    value.is_finite().then_some(value)
}

fn json_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cc_switch_quota_rows() {
        let rate_limit = parse_cc_switch_quota_row("daily\t12.500000\t50.000000\tSub2API").unwrap();
        assert_eq!(rate_limit.used_percent, 25);
        assert_eq!(rate_limit.cost_usd, Some(12.5));
        assert_eq!(rate_limit.remaining_usd, Some(37.5));
        assert_eq!(rate_limit.limit_usd, Some(50.0));
        assert_eq!(
            rate_limit.limit_label.as_deref(),
            Some("cc-switch daily Sub2API")
        );
    }

    #[test]
    fn parses_cc_switch_rows_without_limits() {
        let rate_limit = parse_cc_switch_quota_row("none\t0.000000\t0.000000\tSub2API").unwrap();
        assert_eq!(rate_limit.used_percent, 0);
        assert_eq!(rate_limit.cost_usd, None);
        assert_eq!(rate_limit.remaining_usd, None);
        assert_eq!(rate_limit.limit_usd, None);
        assert_eq!(
            rate_limit.limit_label.as_deref(),
            Some("cc-switch no limit Sub2API")
        );
    }

    #[test]
    fn parses_usage_script_from_provider_meta() {
        let meta = r#"{
            "usage_script": {
                "enabled": true,
                "baseUrl": "http://127.0.0.1:8089/",
                "apiKey": "sk-test",
                "timeout": 7
            }
        }"#;

        let script = parse_usage_script(meta).unwrap();

        assert_eq!(script.base_url, "http://127.0.0.1:8089");
        assert_eq!(script.api_key, "sk-test");
        assert_eq!(script.timeout_secs, 7);
    }

    #[test]
    fn parses_usage_script_from_snake_case_string_meta() {
        let meta = r#"{
            "usageScript": "{\"enabled\":true,\"base_url\":\"http://127.0.0.1:8089/\",\"api_key\":\"sk-test\"}"
        }"#;

        let script = parse_usage_script(meta).unwrap();

        assert_eq!(script.base_url, "http://127.0.0.1:8089");
        assert_eq!(script.api_key, "sk-test");
        assert_eq!(script.timeout_secs, 10);
    }

    #[test]
    fn falls_back_to_legacy_quota_when_usage_script_fails() {
        let provider = CcSwitchProvider {
            name: "Sub2API".to_string(),
            usage_script: Some(CcSwitchUsageScript {
                base_url: "http://127.0.0.1:8089".to_string(),
                api_key: "sk-test".to_string(),
                timeout_secs: 1,
            }),
        };

        let rate_limit = select_cc_switch_rate_limit(
            Some(&provider),
            |_script, _provider| Err(io::Error::other("usage endpoint unavailable")),
            || {
                Ok(parse_cc_switch_quota_row(
                    "daily\t12.500000\t50.000000\tSub2API",
                ))
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(rate_limit.used_percent, 25);
        assert_eq!(
            rate_limit.limit_label.as_deref(),
            Some("cc-switch daily Sub2API")
        );
    }

    #[test]
    fn parses_cc_switch_usage_response_with_quota_values() {
        let value = serde_json::json!({
            "quota": {
                "limit": 3333,
                "remaining": 3321.900394,
                "unit": "USD",
                "used": 11.099606
            },
            "remaining": 3321.900394,
            "unit": "USD",
            "usage": {
                "today": {
                    "cost": 378.83772075,
                    "actual_cost": 378.83772075
                }
            }
        });

        let rate_limit = parse_cc_switch_usage_response(&value, Some("Sub2API")).unwrap();

        assert_eq!(rate_limit.used_percent, 0);
        assert!((rate_limit.cost_usd.unwrap() - 11.099606).abs() < 1e-9);
        assert!((rate_limit.remaining_usd.unwrap() - 3321.900394).abs() < 1e-9);
        assert!((rate_limit.limit_usd.unwrap() - 3333.0).abs() < 1e-9);
        assert_eq!(
            rate_limit.limit_label.as_deref(),
            Some("cc-switch usage Sub2API")
        );
    }
}
