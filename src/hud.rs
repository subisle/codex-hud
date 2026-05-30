#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenUsage {
    pub used: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitSummary {
    pub used_percent: u8,
    pub limit_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalContext {
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudSnapshot {
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
    pub model: Option<String>,
    pub turn_status: Option<String>,
    pub token_usage: Option<TokenUsage>,
    pub rate_limit: Option<RateLimitSummary>,
    pub local: LocalContext,
    pub goal: Option<String>,
    pub plan: Option<String>,
    pub mcp_summary: Option<String>,
    pub tool_summary: Option<String>,
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
            lines.push(format!("tool: {tool_summary}"));
        }
        if let Some(rate_limit) = self.rate_limit.as_ref() {
            let label = rate_limit.limit_label.as_deref().unwrap_or("unknown");
            lines.push(format!("rate: {}% ({label})", rate_limit.used_percent));
        }

        lines
    }
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if text.len() <= width {
        return text.to_string();
    }

    text.chars().take(width).collect()
}

pub fn fit_line(text: impl AsRef<str>, width: usize) -> String {
    truncate_to_width(text.as_ref(), width)
}
