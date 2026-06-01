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

const RGB_CYAN: (u8, u8, u8) = (8, 233, 255);
const RGB_PURPLE: (u8, u8, u8) = (176, 138, 255);
const RGB_YELLOW: (u8, u8, u8) = (255, 210, 41);
const RGB_GREEN: (u8, u8, u8) = (33, 240, 178);
const RGB_BLUE: (u8, u8, u8) = (93, 162, 255);
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
    let percent = snapshot
        .token_usage
        .as_ref()
        .and_then(|usage| percent_from_usage(usage.used, usage.limit));
    format!("上下文 [{}] {}", plain_bar(percent), percent_label(percent))
}

fn quota_segment(snapshot: &HudSnapshot) -> String {
    let percent = snapshot.rate_limit.as_ref().map(|rate| rate.used_percent);
    format!("$ [{}] {}", plain_bar(percent), percent_label(percent))
}

fn tools_segment(snapshot: &HudSnapshot) -> String {
    format!("MCP x{}", snapshot.mcp_count)
}

fn percent_from_usage(used: u64, limit: u64) -> Option<u8> {
    if limit == 0 {
        return None;
    }

    let percent = used.saturating_mul(100) / limit;
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
        } else if part.starts_with("$") {
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
    colorize_metric_part(part, FG_YELLOW, |index, filled| {
        gradient_cell(index, filled, RGB_GREEN, RGB_YELLOW, RGB_RED)
    })
}

fn colorize_quota_part(part: &str) -> String {
    colorize_metric_part(part, FG_BLUE, |index, filled| {
        gradient_cell(index, filled, RGB_CYAN, RGB_BLUE, RGB_PURPLE)
    })
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

fn gradient_cell(
    index: usize,
    filled: usize,
    start: (u8, u8, u8),
    middle: (u8, u8, u8),
    end: (u8, u8, u8),
) -> (u8, u8, u8) {
    if filled <= 1 {
        return start;
    }

    let ratio = index as f32 / (filled - 1) as f32;
    if ratio <= 0.5 {
        lerp_rgb(start, middle, ratio * 2.0)
    } else {
        lerp_rgb(middle, end, (ratio - 0.5) * 2.0)
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
