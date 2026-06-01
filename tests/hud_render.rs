use codex_hud::hud::{
    apply_app_server_message, HudSnapshot, LocalContext, RateLimitSummary, TokenUsage,
};
use codex_hud::surface::{render_compact_ansi, render_expanded};
use serde_json::json;
use unicode_width::UnicodeWidthStr;

fn snapshot() -> HudSnapshot {
    HudSnapshot {
        thread_id: Some("thr_123".to_string()),
        thread_name: Some("build-agent".to_string()),
        model: Some("gpt-5.4".to_string()),
        model_provider: Some("openai".to_string()),
        turn_status: Some("running".to_string()),
        token_usage: Some(TokenUsage {
            used: 9_216,
            limit: 12_800,
        }),
        rate_limit: Some(RateLimitSummary {
            used_percent: 31,
            cost_usd: Some(15.5),
            remaining_usd: Some(34.5),
            limit_usd: Some(50.0),
            limit_label: Some("codex".to_string()),
        }),
        local: LocalContext {
            cwd: Some("/Users/me/codex-hud".to_string()),
            git_branch: Some("main".to_string()),
            git_dirty: true,
        },
        goal: Some("ship transport layer".to_string()),
        plan: Some("finish task 4".to_string()),
        mcp_summary: Some("2 servers".to_string()),
        tool_summary: Some("Read x3 Edit x1".to_string()),
        mcp_count: 3,
        skill_count: 7,
    }
}

#[test]
fn compact_rendering_stays_dense_and_contains_core_fields() {
    let lines = render_compact_ansi(&snapshot(), 120);
    let visible = normalize_ansi_lines(&lines.join("\n"));

    assert_eq!(visible.len(), 2);
    assert!(lines_fit(&visible, 120));
    assert_eq!(
        visible,
        vec![
            "[GPT-5.4] 来源 openai | codex-hud git:(main*)".to_string(),
            "上下文 [■■■■■■■···] 72% 9.22K/12.8K | 已用 $15.50 余额 $34.50 | MCP x3".to_string(),
        ]
    );
}

#[test]
fn compact_rendering_truncates_instead_of_expanding() {
    let lines = normalize_ansi_lines(&render_compact_ansi(&snapshot(), 48).join("\n"));

    assert!(lines.len() <= 2);
    assert!(lines_fit(&lines, 48));
}

#[test]
fn fit_line_truncates_by_terminal_display_width() {
    assert_eq!(codex_hud::hud::fit_line("中", 2), "中");
    assert_eq!(codex_hud::hud::fit_line("ab中c", 4), "ab中");
}

#[test]
fn fit_line_counts_terminal_columns_without_splitting_utf8() {
    assert_eq!(codex_hud::hud::fit_line("aé", 2), "aé");
    assert_eq!(codex_hud::hud::fit_line("é", 1), "é");
}

#[test]
fn compact_rendering_handles_zero_width() {
    let lines = render_compact_ansi(&snapshot(), 0);

    assert!(lines.is_empty());
}

#[test]
fn compact_rendering_shows_progress_placeholders_when_usage_is_missing() {
    let mut snapshot = snapshot();
    snapshot.token_usage = None;
    snapshot.rate_limit = None;

    let rendered = normalize_ansi_lines(&render_compact_ansi(&snapshot, 100).join("\n")).join("\n");

    assert!(rendered.contains("上下文 [··········] 0%"));
    assert!(!rendered.contains("$ ["));
    assert!(rendered.contains("MCP x3"));
}

#[test]
fn compact_rendering_abbreviates_large_money_values_without_quota_bar() {
    let mut snapshot = snapshot();
    snapshot.rate_limit = Some(RateLimitSummary {
        used_percent: 0,
        cost_usd: Some(439.98),
        remaining_usd: Some(1_112_221_004.39),
        limit_usd: Some(1_112_221_444.37),
        limit_label: Some("Sub2API".to_string()),
    });

    let rendered = normalize_ansi_lines(&render_compact_ansi(&snapshot, 120).join("\n")).join("\n");

    assert!(rendered.contains("已用 $439.98 余额 $1.11B"));
    assert!(!rendered.contains("$ ["));
    assert!(!rendered.contains("1112221004.39"));
}

#[test]
fn compact_rendering_abbreviates_money_across_common_ranges() {
    let cases = [
        (999.99, "$999.99"),
        (2_932.22223232, "$2.93K"),
        (27_333.0, "$27.33K"),
        (1_112_221_004.39, "$1.11B"),
        (7_776_666_666_666_666.0, "$7.78Q"),
        (1_234_000_000_000_000_000.0, "$1.23e18"),
    ];

    for (remaining, expected) in cases {
        let mut snapshot = snapshot();
        snapshot.rate_limit = Some(RateLimitSummary {
            used_percent: 0,
            cost_usd: None,
            remaining_usd: Some(remaining),
            limit_usd: None,
            limit_label: Some("Sub2API".to_string()),
        });

        let rendered =
            normalize_ansi_lines(&render_compact_ansi(&snapshot, 120).join("\n")).join("\n");

        assert!(rendered.contains(&format!("余额 {expected}")), "{rendered}");
    }
}

#[test]
fn compact_rendering_uses_real_context_usage_from_app_server_shape() {
    let mut snapshot = snapshot();
    snapshot.token_usage = None;
    snapshot.rate_limit = None;

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "thr_123",
                "turnId": "turn_1",
                "tokenUsage": {
                    "total": {
                        "totalTokens": 9216,
                        "inputTokens": 8000,
                        "cachedInputTokens": 1024,
                        "outputTokens": 1000,
                        "reasoningOutputTokens": 216
                    },
                    "last": {
                        "totalTokens": 2048,
                        "inputTokens": 1800,
                        "cachedInputTokens": 100,
                        "outputTokens": 200,
                        "reasoningOutputTokens": 48
                    },
                    "modelContextWindow": 128000
                }
            }
        })
    ));

    let rendered = normalize_ansi_lines(&render_compact_ansi(&snapshot, 100).join("\n")).join("\n");

    assert!(rendered.contains("上下文 [··········] 1%"));
    assert!(rendered.contains("9.22K/1.05M"));
    assert!(!rendered.contains("上下文 [··········] --"));
}

#[test]
fn compact_ansi_rendering_adds_terminal_color_without_changing_content() {
    let plain = vec![
        "[GPT-5.4] 来源 openai | codex-hud git:(main*)".to_string(),
        "上下文 [■■■■■■■···] 72% 9.22K/12.8K | 已用 $15.50 余额 $34.50 | MCP x3".to_string(),
    ];
    let colored = render_compact_ansi(&snapshot(), 100);
    let colored_joined = colored.join("\n");

    assert!(colored[0].contains("\x1b[48;2;11;16;32m"));
    assert!(colored[0].contains("\x1b[38;2;8;233;255m"));
    assert!(colored[0].contains("\x1b[38;2;176;138;255m"));
    assert!(colored[1].contains("\x1b[38;2;"));
    assert!(colored[1].contains("\x1b[38;2;93;162;255m$15.50"));
    assert!(colored[1].contains("\x1b[38;2;93;162;255m$34.50"));
    assert_eq!(normalize_ansi_lines(&colored_joined), plain);
}

#[test]
fn compact_context_progress_does_not_turn_red_at_low_usage() {
    let mut snapshot = snapshot();
    snapshot.token_usage = Some(TokenUsage {
        used: 1_700,
        limit: 10_000,
    });

    let rendered = render_compact_ansi(&snapshot, 120).join("\n");

    assert!(normalize_ansi_lines(&rendered).join("\n").contains("17%"));
    assert!(!rendered.contains("\x1b[38;2;255;79;103m"));
}

#[test]
fn compact_rendering_truncates_by_terminal_display_width() {
    let mut snapshot = snapshot();
    snapshot.model = Some("ab中c".to_string());
    snapshot.thread_name = None;
    snapshot.turn_status = None;
    snapshot.token_usage = None;
    snapshot.rate_limit = None;
    snapshot.local = LocalContext {
        cwd: None,
        git_branch: None,
        git_dirty: false,
    };
    snapshot.thread_id = None;

    let lines = normalize_ansi_lines(&render_compact_ansi(&snapshot, 4).join("\n"));

    assert_eq!(lines[0], "[AB");
    assert!(lines_fit(&lines, 4));
}

#[test]
fn expanded_rendering_truncates_by_terminal_display_width() {
    let snapshot = HudSnapshot {
        thread_id: None,
        thread_name: None,
        model: None,
        model_provider: None,
        turn_status: None,
        token_usage: None,
        rate_limit: None,
        local: LocalContext {
            cwd: None,
            git_branch: None,
            git_dirty: false,
        },
        goal: Some("ab中c".to_string()),
        plan: None,
        mcp_summary: None,
        tool_summary: None,
        mcp_count: 0,
        skill_count: 0,
    };

    assert_eq!(
        render_expanded(&snapshot, 10),
        vec!["[codex-hud", "上下文 [··", "goal: ab中"]
    );
}

#[test]
fn expanded_rendering_surfaces_goal_plan_mcp_and_tools() {
    let lines = render_expanded(&snapshot(), 100);
    let rendered = lines.join("\n");

    assert!(rendered.contains("plan"));
    assert!(rendered.contains("goal"));
    assert!(rendered.contains("MCP"));
    assert!(rendered.contains("技能"));
    assert!(rendered.contains("rate"));
}

fn lines_fit(lines: &[String], width: usize) -> bool {
    lines.iter().all(|line| line.width() <= width)
}

fn strip_ansi(text: &str) -> String {
    let mut output = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }

        output.push(ch);
    }

    output
}

fn normalize_ansi_lines(text: &str) -> Vec<String> {
    strip_ansi(text)
        .lines()
        .map(|line| {
            line.trim_end()
                .replace("上下文[", "上下文 [")
                .replace("$[", "$ [")
        })
        .collect()
}
