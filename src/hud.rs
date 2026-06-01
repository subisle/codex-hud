use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenUsage {
    pub used: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitSummary {
    pub used_percent: u8,
    pub cost_usd: Option<f64>,
    pub remaining_usd: Option<f64>,
    pub limit_usd: Option<f64>,
    pub limit_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalContext {
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HudSnapshot {
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub turn_status: Option<String>,
    pub token_usage: Option<TokenUsage>,
    pub rate_limit: Option<RateLimitSummary>,
    pub local: LocalContext,
    pub goal: Option<String>,
    pub plan: Option<String>,
    pub mcp_summary: Option<String>,
    pub tool_summary: Option<String>,
    pub mcp_count: u64,
    pub skill_count: u64,
}

impl HudSnapshot {
    pub fn compact_headline(&self) -> String {
        let mut parts = Vec::new();

        if let Some(model) = self.model.as_deref() {
            parts.push(model.to_string());
        }
        if let Some(thread_name) = self.thread_name.as_deref() {
            parts.push(thread_name.to_string());
        }
        if let Some(turn_status) = self.turn_status.as_deref() {
            parts.push(turn_status.to_string());
        }
        if let Some(usage) = self.token_usage.as_ref() {
            parts.push(format!("ctx {}/{}", usage.used, usage.limit));
        }
        if let Some(rate_limit) = self.rate_limit.as_ref() {
            parts.push(format!("rate {}%", rate_limit.used_percent));
        }

        parts.join(" | ")
    }

    pub fn compact_progress(&self) -> String {
        let parts = [
            progress_segment(
                "ctx",
                self.token_usage
                    .as_ref()
                    .and_then(|usage| usage_percent(usage.used, usage.limit)),
            ),
            progress_segment(
                "usage",
                self.rate_limit
                    .as_ref()
                    .map(|rate_limit| rate_limit.used_percent),
            ),
        ];

        parts.join(" | ")
    }

    pub fn merge_rate_limit(&mut self, incoming: RateLimitSummary) -> bool {
        match self.rate_limit.as_mut() {
            Some(current) => {
                let merged = RateLimitSummary {
                    used_percent: incoming.used_percent,
                    cost_usd: incoming.cost_usd.or(current.cost_usd),
                    remaining_usd: incoming.remaining_usd.or(current.remaining_usd),
                    limit_usd: incoming.limit_usd.or(current.limit_usd),
                    limit_label: prefer_limit_label(
                        current.limit_label.clone(),
                        incoming.limit_label,
                    ),
                };

                if *current == merged {
                    false
                } else {
                    *current = merged;
                    true
                }
            }
            None => {
                self.rate_limit = Some(incoming);
                true
            }
        }
    }

    pub fn compact_context(&self) -> String {
        let mut parts = Vec::new();

        if let Some(cwd) = self.local.cwd.as_deref() {
            parts.push(cwd.to_string());
        }
        if let Some(branch) = self.local.git_branch.as_deref() {
            let branch = if self.local.git_dirty {
                format!("{branch}*")
            } else {
                branch.to_string()
            };
            parts.push(branch);
        }
        if let Some(thread_id) = self.thread_id.as_deref() {
            parts.push(format!("#{thread_id}"));
        }

        parts.join(" | ")
    }

    pub fn expanded_details(&self) -> Vec<String> {
        let mut lines = Vec::new();

        if let Some(goal) = self.goal.as_deref() {
            lines.push(format!("goal: {goal}"));
        }
        if let Some(plan) = self.plan.as_deref() {
            lines.push(format!("plan: {plan}"));
        }
        if let Some(mcp_summary) = self.mcp_summary.as_deref() {
            lines.push(format!("MCP: {mcp_summary}"));
        }
        if let Some(tool_summary) = self.tool_summary.as_deref() {
            lines.push(format!("技能: {tool_summary}"));
        }
        if let Some(rate_limit) = self.rate_limit.as_ref() {
            let label = rate_limit.limit_label.as_deref().unwrap_or("unknown");
            lines.push(format!("rate: {}% ({label})", rate_limit.used_percent));
        }

        lines
    }
}

pub fn apply_app_server_message(snapshot: &mut HudSnapshot, message: &Value) -> bool {
    let Some(object) = message.as_object() else {
        return false;
    };

    if let Some(method) = object.get("method").and_then(Value::as_str) {
        let payload = object
            .get("params")
            .or_else(|| object.get("result"))
            .unwrap_or(&Value::Null);

        return match method {
            "thread/started" => apply_thread_payload(snapshot, payload, true),
            "thread/updated" | "thread/name/updated" => {
                apply_thread_payload(snapshot, payload, false)
            }
            "thread/status/changed" => apply_thread_status(snapshot, payload),
            "thread/tokenUsage/updated" => apply_token_usage(snapshot, payload),
            "thread/start" | "turn/start" | "turn/steer" => {
                apply_skill_invocations(snapshot, payload)
            }
            "turn/started" | "turn/interrupt" | "turn/completed" => {
                apply_turn_status(snapshot, method, payload)
            }
            "turn/plan/updated" => apply_plan(snapshot, payload),
            "item/started" | "item/updated" | "item/completed" => apply_item(snapshot, payload),
            "thread/goal/set" | "thread/goal/updated" | "thread/goal/get" => {
                apply_goal(snapshot, payload)
            }
            "thread/goal/clear" | "thread/goal/cleared" => {
                snapshot.goal = None;
                true
            }
            "account/rateLimits/read" | "account/rateLimits/updated" => {
                apply_rate_limit(snapshot, payload)
            }
            _ => false,
        };
    }

    if let Some(result) = object.get("result") {
        let mut updated = false;

        if let Some(thread) = result.get("thread") {
            updated |= apply_thread_payload(snapshot, thread, true);
        }
        if let Some(rate_limits) = result.get("rateLimits") {
            updated |= apply_rate_limit(snapshot, rate_limits);
        }
        if let Some(token_usage) = result.get("tokenUsage") {
            updated |= apply_token_usage(snapshot, token_usage);
        }
        if let Some(goal) = result.get("goal") {
            updated |= apply_goal(snapshot, goal);
        }

        return updated;
    }

    false
}

fn apply_thread_payload(snapshot: &mut HudSnapshot, payload: &Value, allow_rebind: bool) -> bool {
    let payload = thread_object(payload);
    let Some(object) = payload.as_object() else {
        return false;
    };

    if !allow_rebind && !thread_id_matches(snapshot, object) {
        return false;
    }

    let mut updated = false;
    let previous_thread_id = snapshot.thread_id.clone();
    updated |= set_string_field(
        &mut snapshot.thread_id,
        object,
        &["threadId", "thread_id", "id"],
    );
    if previous_thread_id.as_deref() != snapshot.thread_id.as_deref() {
        reset_thread_scoped_state(snapshot);
        updated = true;
    }
    updated |= set_string_field(
        &mut snapshot.thread_name,
        object,
        &["name", "threadName", "thread_name", "title"],
    );
    updated |= set_string_field(
        &mut snapshot.model,
        object,
        &["model", "modelName", "model_name"],
    );
    updated |= set_string_field(
        &mut snapshot.model_provider,
        object,
        &["modelProvider", "model_provider", "provider"],
    );
    updated |= set_string_field(
        &mut snapshot.turn_status,
        object,
        &["status", "turnStatus", "turn_status"],
    );
    updated |= apply_token_usage(snapshot, payload);

    updated
}

fn reset_thread_scoped_state(snapshot: &mut HudSnapshot) {
    snapshot.thread_name = None;
    snapshot.turn_status = None;
    snapshot.token_usage = None;
    snapshot.goal = None;
    snapshot.plan = None;
    snapshot.tool_summary = None;
    snapshot.skill_count = 0;
}

fn apply_thread_status(snapshot: &mut HudSnapshot, payload: &Value) -> bool {
    let payload = thread_object(payload);
    let Some(object) = payload.as_object() else {
        return false;
    };

    if !thread_id_matches(snapshot, object) {
        return false;
    }

    set_string_field(
        &mut snapshot.turn_status,
        object,
        &["status", "turnStatus", "turn_status"],
    )
}

fn apply_token_usage(snapshot: &mut HudSnapshot, payload: &Value) -> bool {
    let payload = thread_object(payload);
    let Some(object) = payload.as_object() else {
        return false;
    };

    if !thread_id_matches(snapshot, object) {
        return false;
    }

    let candidate = object
        .get("tokenUsage")
        .or_else(|| object.get("usage"))
        .or_else(|| object.get("context"));
    let Some(token_usage) =
        candidate.and_then(|candidate| parse_token_usage(candidate, snapshot.model.as_deref()))
    else {
        return false;
    };

    snapshot.token_usage = Some(token_usage);
    true
}

fn apply_skill_invocations(snapshot: &mut HudSnapshot, payload: &Value) -> bool {
    let count = count_skill_inputs(payload);
    if count == 0 {
        return false;
    }

    snapshot.skill_count = snapshot.skill_count.saturating_add(count as u64);
    true
}

fn apply_rate_limit(snapshot: &mut HudSnapshot, payload: &Value) -> bool {
    let Some(object) = payload.as_object() else {
        return false;
    };

    let candidate = object
        .get("rateLimits")
        .or_else(|| object.get("rateLimit"))
        .unwrap_or(payload);
    let Some(rate_limit) = parse_rate_limit(candidate) else {
        return false;
    };

    snapshot.merge_rate_limit(rate_limit)
}

fn apply_goal(snapshot: &mut HudSnapshot, payload: &Value) -> bool {
    let Some(object) = payload.as_object() else {
        return false;
    };

    if let Some(goal) = object
        .get("goal")
        .or_else(|| object.get("text"))
        .or_else(|| object.get("value"))
        .and_then(Value::as_str)
    {
        snapshot.goal = Some(goal.to_string());
        return true;
    }

    false
}

fn apply_turn_status(snapshot: &mut HudSnapshot, method: &str, payload: &Value) -> bool {
    let payload = thread_object(payload);
    let Some(object) = payload.as_object() else {
        return false;
    };

    if !thread_id_matches(snapshot, object) {
        return false;
    }

    let status = object
        .get("status")
        .or_else(|| object.get("state"))
        .or_else(|| object.get("phase"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| match method {
            "turn/started" => "running".to_string(),
            "turn/interrupt" => "interrupted".to_string(),
            "turn/completed" => "completed".to_string(),
            _ => method.to_string(),
        });

    if snapshot.turn_status.as_deref() == Some(status.as_str()) {
        return false;
    }

    snapshot.turn_status = Some(status);
    true
}

fn apply_item(snapshot: &mut HudSnapshot, payload: &Value) -> bool {
    let payload = thread_object(payload);
    let Some(object) = payload.as_object() else {
        return false;
    };

    if !thread_id_matches(snapshot, object) {
        return false;
    }

    let item_type = object
        .get("type")
        .or_else(|| object.get("kind"))
        .or_else(|| object.get("itemType"))
        .and_then(Value::as_str);
    let title = object
        .get("title")
        .or_else(|| object.get("name"))
        .or_else(|| object.get("toolName"))
        .or_else(|| object.get("command"))
        .and_then(Value::as_str);
    let status = object
        .get("status")
        .or_else(|| object.get("state"))
        .and_then(Value::as_str);

    let mut parts = Vec::new();
    if let Some(item_type) = item_type {
        parts.push(item_type.to_string());
    }
    if let Some(title) = title {
        parts.push(title.to_string());
    }
    if let Some(status) = status {
        parts.push(status.to_string());
    }

    if parts.is_empty() {
        return false;
    }

    let summary = parts.join(" ");
    if snapshot.tool_summary.as_deref() == Some(summary.as_str()) {
        return false;
    }

    snapshot.tool_summary = Some(summary);
    true
}

fn count_skill_inputs(value: &Value) -> usize {
    match value {
        Value::Array(items) => items.iter().map(count_skill_inputs).sum(),
        Value::Object(object) => {
            let current = if object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|item_type| item_type == "skill")
            {
                1
            } else {
                0
            };
            current + object.values().map(count_skill_inputs).sum::<usize>()
        }
        _ => 0,
    }
}

fn apply_plan(snapshot: &mut HudSnapshot, payload: &Value) -> bool {
    let Some(object) = payload.as_object() else {
        return false;
    };

    let Some(plan) = object.get("plan").and_then(Value::as_array) else {
        return false;
    };

    let mut parts = Vec::new();
    for item in plan {
        let Some(item_object) = item.as_object() else {
            continue;
        };

        let label = item_object
            .get("step")
            .or_else(|| item_object.get("title"))
            .or_else(|| item_object.get("text"))
            .and_then(Value::as_str);
        let status = item_object
            .get("status")
            .or_else(|| item_object.get("state"))
            .and_then(Value::as_str);

        match (label, status) {
            (Some(label), Some(status)) => parts.push(format!("{label}: {status}")),
            (Some(label), None) => parts.push(label.to_string()),
            _ => {}
        }
    }

    if parts.is_empty() {
        return false;
    }

    snapshot.plan = Some(parts.join("; "));
    true
}

fn thread_object(payload: &Value) -> &Value {
    payload
        .as_object()
        .and_then(|object| object.get("thread"))
        .unwrap_or(payload)
}

fn thread_id_matches(snapshot: &HudSnapshot, object: &serde_json::Map<String, Value>) -> bool {
    let message_thread_id = object
        .get("threadId")
        .or_else(|| object.get("thread_id"))
        .or_else(|| object.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);

    match (snapshot.thread_id.as_deref(), message_thread_id.as_deref()) {
        (Some(current), Some(candidate)) => current == candidate,
        (Some(_), None) | (None, _) => true,
    }
}

fn parse_token_usage(value: &Value, model: Option<&str>) -> Option<TokenUsage> {
    let object = value.as_object()?;
    if let Some(used) = object.get("used").and_then(parse_u64) {
        if let Some(limit) = token_limit_from_payload_or_model(object, model) {
            return Some(TokenUsage { used, limit });
        }
    }

    if let Some(used) = live_context_tokens(object) {
        let limit = token_limit_from_payload_or_model(object, model)?;
        return Some(TokenUsage { used, limit });
    }

    let total = object.get("total").and_then(Value::as_object);
    let used = total
        .and_then(|total| total.get("totalTokens"))
        .or_else(|| object.get("totalTokens"))
        .and_then(parse_u64)?;
    let limit = token_limit_from_payload_or_model(object, model)?;

    Some(TokenUsage { used, limit })
}

fn live_context_tokens(object: &serde_json::Map<String, Value>) -> Option<u64> {
    for key in [
        "current",
        "currentContext",
        "current_context",
        "live",
        "liveContext",
        "live_context",
        "contextUsage",
        "context_usage",
        "context",
    ] {
        if let Some(used) = object.get(key).and_then(token_count_from_value) {
            return Some(used);
        }
    }

    None
}

fn token_count_from_value(value: &Value) -> Option<u64> {
    if let Some(tokens) = parse_u64(value) {
        return Some(tokens);
    }

    let object = value.as_object()?;
    object
        .get("totalTokens")
        .or_else(|| object.get("total_tokens"))
        .or_else(|| object.get("tokens"))
        .or_else(|| object.get("used"))
        .and_then(parse_u64)
}

fn token_limit_from_payload_or_model(
    object: &serde_json::Map<String, Value>,
    model: Option<&str>,
) -> Option<u64> {
    let explicit = object
        .get("modelContextWindow")
        .or_else(|| object.get("contextWindow"))
        .or_else(|| object.get("limit"))
        .or_else(|| object.get("max"))
        .and_then(parse_u64);
    let model_window = model.and_then(model_context_window);

    match (explicit, model_window) {
        (Some(explicit), Some(model_window)) => Some(explicit.max(model_window)),
        (Some(explicit), None) => Some(explicit),
        (None, Some(model_window)) => Some(model_window),
        (None, None) => None,
    }
}

fn model_context_window(model: &str) -> Option<u64> {
    let model = model.to_ascii_lowercase();
    let model = model.strip_prefix("openai.").unwrap_or(&model);
    let model = model.strip_prefix("openai/").unwrap_or(model);

    if model.starts_with("gpt-5.5")
        || model.starts_with("gpt-5.4-pro")
        || model.starts_with("gpt-5.4")
    {
        if model.starts_with("gpt-5.4-mini") || model.starts_with("gpt-5.4-nano") {
            Some(400_000)
        } else {
            Some(1_050_000)
        }
    } else if model.starts_with("gpt-5-mini") || model.starts_with("gpt-5-nano") {
        Some(400_000)
    } else {
        None
    }
}

fn parse_rate_limit(value: &Value) -> Option<RateLimitSummary> {
    let object = value.as_object()?;
    let quota = object.get("quota").and_then(Value::as_object);
    let usage = object.get("usage").and_then(Value::as_object);
    let today = usage.and_then(|usage| usage.get("today").and_then(Value::as_object));

    let cost_usd = object
        .get("cost")
        .or_else(|| object.get("actualCost"))
        .or_else(|| object.get("actual_cost"))
        .and_then(parse_f64)
        .or_else(|| object.get("used").and_then(parse_f64))
        .or_else(|| {
            quota
                .and_then(|quota| quota.get("used"))
                .and_then(parse_f64)
        })
        .or_else(|| {
            today
                .and_then(|today| today.get("actual_cost"))
                .and_then(parse_f64)
        })
        .or_else(|| {
            today
                .and_then(|today| today.get("cost"))
                .and_then(parse_f64)
        });
    let remaining_usd = object
        .get("remaining")
        .or_else(|| object.get("balance"))
        .and_then(parse_f64)
        .or_else(|| {
            quota
                .and_then(|quota| quota.get("remaining"))
                .and_then(parse_f64)
        })
        .or_else(|| {
            quota
                .and_then(|quota| quota.get("balance"))
                .and_then(parse_f64)
        });
    let limit_usd = object.get("limit").and_then(parse_f64).or_else(|| {
        quota
            .and_then(|quota| quota.get("limit"))
            .and_then(parse_f64)
    });
    let used_percent = object
        .get("usedPercent")
        .or_else(|| object.get("used_percent"))
        .or_else(|| object.get("usagePercent"))
        .or_else(|| object.get("percentage"))
        .or_else(|| {
            object
                .get("primary")
                .and_then(Value::as_object)
                .and_then(|primary| {
                    primary
                        .get("usedPercent")
                        .or_else(|| primary.get("used_percent"))
                        .or_else(|| primary.get("usagePercent"))
                        .or_else(|| primary.get("percentage"))
                })
        })
        .and_then(parse_u8)
        .or_else(|| derived_percent(cost_usd, remaining_usd, limit_usd))?;
    let limit_label = object
        .get("limitName")
        .or_else(|| object.get("limitId"))
        .or_else(|| object.get("name"))
        .or_else(|| {
            object
                .get("primary")
                .and_then(Value::as_object)
                .and_then(|primary| primary.get("limitName").or_else(|| primary.get("limitId")))
        })
        .and_then(Value::as_str)
        .map(str::to_string);

    Some(RateLimitSummary {
        used_percent,
        cost_usd,
        remaining_usd,
        limit_usd,
        limit_label,
    })
}

fn set_string_field(
    field: &mut Option<String>,
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> bool {
    for key in keys {
        if let Some(value) = object.get(*key).and_then(Value::as_str) {
            let next = value.to_string();
            if field.as_deref() == Some(next.as_str()) {
                return false;
            }
            *field = Some(next);
            return true;
        }
    }

    false
}

fn parse_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn parse_u8(value: &Value) -> Option<u8> {
    let value = parse_u64(value)?;
    u8::try_from(value).ok()
}

fn parse_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn derived_percent(
    cost_usd: Option<f64>,
    remaining_usd: Option<f64>,
    limit_usd: Option<f64>,
) -> Option<u8> {
    let limit = limit_usd?;
    if limit <= 0.0 {
        return None;
    }

    let used = cost_usd.or_else(|| remaining_usd.map(|remaining| (limit - remaining).max(0.0)))?;
    let percent = ((used / limit) * 100.0).round().clamp(0.0, 100.0);
    Some(percent as u8)
}

fn prefer_limit_label(current: Option<String>, incoming: Option<String>) -> Option<String> {
    match (current, incoming) {
        (Some(current), Some(incoming))
            if current.starts_with("cc-switch") && !incoming.starts_with("cc-switch") =>
        {
            Some(current)
        }
        (_, Some(incoming)) => Some(incoming),
        (Some(current), None) => Some(current),
        (None, None) => None,
    }
}

fn usage_percent(used: u64, limit: u64) -> Option<u8> {
    if limit == 0 {
        return None;
    }

    let percent = ((u128::from(used) * 100) + (u128::from(limit) / 2)) / u128::from(limit);
    u8::try_from(percent.min(100)).ok()
}

fn progress_segment(label: &str, percent: Option<u8>) -> String {
    match percent {
        Some(percent) => format!("{label} {percent}% {}", progress_bar(percent, 10)),
        None => format!("{label} 0% {}", progress_bar(0, 10)),
    }
}

fn progress_bar(percent: u8, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let filled = (usize::from(percent.min(100)) * width + 50) / 100;
    let filled = filled.min(width);

    let mut bar = String::with_capacity(width);
    for _ in 0..filled {
        bar.push('█');
    }
    for _ in filled..width {
        bar.push('░');
    }
    bar
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    if text.width() <= width {
        return text.to_string();
    }

    let mut current_width = 0;
    let mut end = 0;
    for (start, ch) in text.char_indices() {
        let char_width = ch.width().unwrap_or(0);
        let next_width = current_width + char_width;
        if next_width > width {
            break;
        }
        current_width = next_width;
        end = start + ch.len_utf8();
    }

    text[..end].to_string()
}

pub fn fit_line(text: impl AsRef<str>, width: usize) -> String {
    truncate_to_width(text.as_ref(), width)
}
