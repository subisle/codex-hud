use crate::hud::{fit_line, HudSnapshot};

const ANSI_RESET: &str = "\x1b[0m";
const FG_TEXT: &str = "\x1b[38;2;157;168;189m";
const FG_MUTED: &str = "\x1b[38;2;116;128;150m";
const FG_CYAN: &str = "\x1b[38;2;8;233;255m";
const FG_PURPLE: &str = "\x1b[38;2;176;138;255m";
const FG_YELLOW: &str = "\x1b[38;2;255;210;41m";
const FG_GREEN: &str = "\x1b[38;2;33;240;178m";
const FG_BLUE: &str = "\x1b[38;2;93;162;255m";
const FG_SEP: &str = "\x1b[38;2;99;112;134m";
const BG_HUD: &str = "\x1b[48;2;11;16;32m";
const BOLD: &str = "\x1b[1m";

const RGB_YELLOW: (u8, u8, u8) = (255, 210, 41);
const RGB_GREEN: (u8, u8, u8) = (33, 240, 178);
const RGB_RED: (u8, u8, u8) = (255, 79, 103);

const SOURCE_LABEL: &str = "来源";
const HUD_TITLE: &str = "codex-hud";
const BAR_WIDTH: usize = 10;

pub fn render_compact(snapshot: &HudSnapshot, width: usize) -> Vec<String> {
    render_compact_preview(snapshot, width)
}

pub fn render_compact_ansi(snapshot: &HudSnapshot, width: usize) -> Vec<String> {
    render_compact_preview_ansi(snapshot, width)
}

pub fn render_compact_preview_ansi(snapshot: &HudSnapshot, width: usize) -> Vec<String> {
    render_compact_preview(snapshot, width)
        .into_iter()
        .enumerate()
        .map(|(index, line)| match index {
            0 => colorize_top_line(&line),
            1 => colorize_bottom_line(&line),
            _ => format!("{BG_HUD}{FG_TEXT}{line}{ANSI_RESET}"),
        })
        .collect()
}

pub fn render_expanded(snapshot: &HudSnapshot, width: usize) -> Vec<String> {
    let mut lines = render_compact_preview(snapshot, width);
    for detail in snapshot.expanded_details() {
        lines.push(fit_line(detail, width));
    }
    lines
}

fn join_non_empty(parts: &[String]) -> String {
    parts
        .iter()
        .map(String::as_str)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn render_compact_preview(snapshot: &HudSnapshot, width: usize) -> Vec<String> {
    let mut lines = vec![
        fit_line(top_line(snapshot), width),
        fit_line(
            join_non_empty(&[
                context_segment(snapshot),
                quota_segment(snapshot),
                tools_segment(snapshot),
            ]),
            width,
        ),
    ];

    lines.retain(|line| !line.is_empty());
    if lines.len() > 2 {
        lines.truncate(2);
    }
    lines
}

fn top_line(snapshot: &HudSnapshot) -> String {
    let model = model_segment(snapshot);
    let source = source_segment(snapshot);
    let git = git_segment(snapshot);
    format!("{model} {source} | {HUD_TITLE} {git}")
}

fn model_segment(snapshot: &HudSnapshot) -> String {
    match snapshot.model.as_deref() {
        Some(model) => format!("[{}]", model.to_ascii_uppercase()),
        None => "[codex-hud]".to_string(),
    }
}

fn source_segment(snapshot: &HudSnapshot) -> String {
    let provider = snapshot.model_provider.as_deref().unwrap_or("--");
    format!("{SOURCE_LABEL} {provider}")
}

fn git_segment(snapshot: &HudSnapshot) -> String {
    if let Some(branch) = snapshot.local.git_branch.as_deref() {
        if snapshot.local.git_dirty {
            format!("git:({branch}*)")
        } else {
            format!("git:({branch})")
        }
    } else {
        "git:(-)".to_string()
    }
}

fn context_segment(snapshot: &HudSnapshot) -> String {
    let usage = snapshot.token_usage.as_ref();
    let percent = usage.and_then(|usage| percent_from_usage(usage.used, usage.limit));
    let usage_label = usage
        .map(|usage| format!(" {}", token_usage_label(usage.used, usage.limit)))
        .unwrap_or_default();

    format!(
        "上下文 [{}] {}{}",
        plain_bar(percent),
        percent_label(percent),
        usage_label
    )
}

fn quota_segment(snapshot: &HudSnapshot) -> String {
    let Some(rate_limit) = snapshot.rate_limit.as_ref() else {
        return String::new();
    };

    let mut parts = Vec::new();
    if let Some(cost) = rate_limit.cost_usd {
        parts.push(format!("已用 {}", money_label(cost)));
    }
    if let Some(remaining) = rate_limit.remaining_usd {
        parts.push(format!("余额 {}", money_label(remaining)));
    }

    parts.join(" ")
}

fn tools_segment(snapshot: &HudSnapshot) -> String {
    format!("MCP x{}", snapshot.mcp_count)
}

fn percent_from_usage(used: u64, limit: u64) -> Option<u8> {
    if limit == 0 {
        return None;
    }

    let percent = ((u128::from(used) * 100) + (u128::from(limit) / 2)) / u128::from(limit);
    u8::try_from(percent.min(100)).ok()
}

fn percent_label(percent: Option<u8>) -> String {
    percent
        .map(|percent| format!("{percent}%"))
        .unwrap_or_else(|| "0%".to_string())
}

fn plain_bar(percent: Option<u8>) -> String {
    let filled = filled_cells(percent);
    let mut bar = String::with_capacity(BAR_WIDTH);
    for _ in 0..filled {
        bar.push('■');
    }
    for _ in filled..BAR_WIDTH {
        bar.push('·');
    }
    bar
}

fn filled_cells(percent: Option<u8>) -> usize {
    if let Some(percent) = percent {
        ((usize::from(percent.min(100)) * BAR_WIDTH) + 50) / 100
    } else {
        0
    }
}

fn colorize_top_line(line: &str) -> String {
    let mut output = String::new();
    let Some((model, rest)) = line.split_once(' ') else {
        return bg(&style(line, FG_CYAN, true));
    };

    output.push_str(&style(model, FG_CYAN, true));
    output.push(' ');

    if let Some((source_side, title_side)) = rest.split_once(" | ") {
        output.push_str(&style(SOURCE_LABEL, FG_TEXT, false));
        output.push(' ');
        output.push_str(&style(
            source_side
                .strip_prefix(SOURCE_LABEL)
                .map(str::trim)
                .unwrap_or(source_side),
            FG_PURPLE,
            true,
        ));
        output.push_str(&sep());
        output.push_str(&colorize_title_git(title_side));
    } else {
        output.push_str(&style(rest, FG_TEXT, false));
    }

    bg(&output)
}

fn colorize_title_git(text: &str) -> String {
    if let Some((title, git)) = text.split_once(" git:") {
        format!(
            "{} {}{}",
            style(title, FG_YELLOW, true),
            style("git:", FG_TEXT, false),
            style(git, FG_PURPLE, true)
        )
    } else {
        style(text, FG_YELLOW, true)
    }
}

fn colorize_bottom_line(line: &str) -> String {
    let parts: Vec<&str> = line.split(" | ").collect();
    let mut output = Vec::new();
    for part in parts {
        if part.starts_with("上下文") {
            output.push(colorize_context_part(part));
        } else if part.starts_with("已用") || part.starts_with("余额") {
            output.push(colorize_quota_part(part));
        } else if part.starts_with("MCP") {
            output.push(colorize_tools_part(part));
        } else {
            output.push(style(part, FG_TEXT, false));
        }
    }

    bg(&output.join(&sep()))
}

fn colorize_context_part(part: &str) -> String {
    colorize_metric_part(part, FG_YELLOW, context_cell_color)
}

fn colorize_quota_part(part: &str) -> String {
    let tokens: Vec<&str> = part.split_whitespace().collect();
    if tokens.is_empty() {
        return style(part, FG_TEXT, false);
    }

    let mut output = String::new();
    let mut index = 0;
    while index < tokens.len() {
        if index > 0 {
            output.push(' ');
        }

        let token = tokens[index];
        if (token == "已用" || token == "余额") && index + 1 < tokens.len() {
            output.push_str(&style(token, FG_TEXT, false));
            output.push(' ');
            output.push_str(&style(tokens[index + 1], FG_BLUE, true));
            index += 2;
        } else {
            output.push_str(&style(token, FG_TEXT, false));
            index += 1;
        }
    }

    output
}

fn colorize_metric_part(
    part: &str,
    value_color: &str,
    cell_color: impl Fn(usize, usize) -> (u8, u8, u8),
) -> String {
    let Some((label, rest)) = part.split_once('[') else {
        return style(part, FG_TEXT, false);
    };
    let Some((bar, suffix)) = rest.split_once(']') else {
        return style(part, FG_TEXT, false);
    };

    let filled = bar.chars().filter(|ch| *ch == '■').count();
    let mut styled_bar = String::new();
    for (index, cell) in bar.chars().enumerate() {
        let text = if cell == '■' { "■" } else { "·" };
        if cell == '■' {
            styled_bar.push_str(&style_rgb(text, cell_color(index, filled), false));
        } else {
            styled_bar.push_str(&style(text, FG_MUTED, false));
        }
    }

    format!(
        "{}[{}]{}",
        style(label, FG_TEXT, false),
        styled_bar,
        style(suffix, value_color, true)
    )
}

fn colorize_tools_part(part: &str) -> String {
    let Some(count) = part.strip_prefix("MCP ") else {
        return style(part, FG_GREEN, true);
    };

    format!(
        "{} {}",
        style("MCP", FG_TEXT, false),
        style(count.trim(), FG_GREEN, true)
    )
}

fn context_cell_color(index: usize, _filled: usize) -> (u8, u8, u8) {
    let position = (index + 1) as f32 / BAR_WIDTH as f32;
    if position <= 0.8 {
        lerp_rgb(RGB_GREEN, RGB_YELLOW, position / 0.8)
    } else {
        lerp_rgb(RGB_YELLOW, RGB_RED, (position - 0.8) / 0.2)
    }
}

fn lerp_rgb(start: (u8, u8, u8), end: (u8, u8, u8), ratio: f32) -> (u8, u8, u8) {
    let ratio = ratio.clamp(0.0, 1.0);
    (
        lerp_channel(start.0, end.0, ratio),
        lerp_channel(start.1, end.1, ratio),
        lerp_channel(start.2, end.2, ratio),
    )
}

fn lerp_channel(start: u8, end: u8, ratio: f32) -> u8 {
    (start as f32 + (end as f32 - start as f32) * ratio).round() as u8
}

fn sep() -> String {
    format!(" {} ", style("|", FG_SEP, false))
}

fn bg(text: &str) -> String {
    format!("{BG_HUD}{text}{ANSI_RESET}")
}

fn style(text: &str, fg: &str, bold: bool) -> String {
    if text.is_empty() {
        return String::new();
    }

    if bold {
        format!("{BOLD}{fg}{text}{ANSI_RESET}{BG_HUD}")
    } else {
        format!("{fg}{text}{ANSI_RESET}{BG_HUD}")
    }
}

fn style_rgb(text: &str, rgb: (u8, u8, u8), bold: bool) -> String {
    let fg = format!("\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2);
    style(text, &fg, bold)
}

fn money_label(amount: f64) -> String {
    let sign = if amount.is_sign_negative() { "-" } else { "" };
    let amount = amount.abs();
    if amount >= 1_000_000_000_000_000_000.0 {
        return format!("{sign}${amount:.2e}");
    }

    let (scaled, unit) = if amount >= 1_000_000_000_000_000.0 {
        (amount / 1_000_000_000_000_000.0, "Q")
    } else if amount >= 1_000_000_000_000.0 {
        (amount / 1_000_000_000_000.0, "T")
    } else if amount >= 1_000_000_000.0 {
        (amount / 1_000_000_000.0, "B")
    } else if amount >= 1_000_000.0 {
        (amount / 1_000_000.0, "M")
    } else if amount >= 1_000.0 {
        (amount / 1_000.0, "K")
    } else {
        (amount, "")
    };

    format!("{sign}${scaled:.2}{unit}")
}

fn token_usage_label(used: u64, limit: u64) -> String {
    format!("{}/{}", compact_u64(used), compact_u64(limit))
}

fn compact_u64(value: u64) -> String {
    if value >= 1_000_000_000 {
        format_scaled(value, 1_000_000_000, "B")
    } else if value >= 1_000_000 {
        format_scaled(value, 1_000_000, "M")
    } else if value >= 1_000 {
        format_scaled(value, 1_000, "K")
    } else {
        value.to_string()
    }
}

fn format_scaled(value: u64, unit: u64, suffix: &str) -> String {
    let scaled = value as f64 / unit as f64;
    let formatted = if scaled >= 100.0 {
        format!("{scaled:.0}")
    } else if scaled >= 10.0 {
        format!("{scaled:.1}")
    } else {
        format!("{scaled:.2}")
    };

    format!(
        "{}{suffix}",
        formatted.trim_end_matches('0').trim_end_matches('.')
    )
}
