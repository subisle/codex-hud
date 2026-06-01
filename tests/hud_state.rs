use codex_hud::hud::{
    apply_app_server_message, HudSnapshot, LocalContext, RateLimitSummary, TokenUsage,
};
use serde_json::json;

fn blank_snapshot() -> HudSnapshot {
    HudSnapshot {
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
        goal: None,
        plan: None,
        mcp_summary: None,
        tool_summary: None,
        mcp_count: 0,
        skill_count: 0,
    }
}

#[test]
fn applies_thread_and_rate_limit_fields_from_app_server_messages() {
    let mut snapshot = blank_snapshot();

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/updated",
            "params": {
                "threadId": "thr_123",
                "name": "build-agent",
                "status": "running",
                "tokenUsage": {
                    "used": 9200,
                    "limit": 12800
                }
            }
        })
    ));

    assert_eq!(snapshot.thread_id.as_deref(), Some("thr_123"));
    assert_eq!(snapshot.thread_name.as_deref(), Some("build-agent"));
    assert_eq!(snapshot.turn_status.as_deref(), Some("running"));
    assert_eq!(
        snapshot.token_usage,
        Some(TokenUsage {
            used: 9200,
            limit: 12800,
        })
    );

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "id": 2,
            "result": {
                "rateLimits": {
                    "limitId": "codex",
                    "usedPercent": 42
                }
            }
        })
    ));

    assert_eq!(
        snapshot.rate_limit,
        Some(RateLimitSummary {
            used_percent: 42,
            cost_usd: None,
            remaining_usd: None,
            limit_usd: None,
            limit_label: Some("codex".to_string()),
        })
    );
}

#[test]
fn preserves_rich_rate_limit_fields_after_sparse_updates() {
    let mut snapshot = blank_snapshot();

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "account/rateLimits/updated",
            "params": {
                "rateLimits": {
                    "limitId": "cc-switch",
                    "usedPercent": 31,
                    "cost": 15.5,
                    "remaining": 34.5,
                    "limit": 50.0
                }
            }
        })
    ));
    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "account/rateLimits/updated",
            "params": {
                "rateLimits": {
                    "limitId": "codex",
                    "usedPercent": 32
                }
            }
        })
    ));

    assert_eq!(
        snapshot.rate_limit,
        Some(RateLimitSummary {
            used_percent: 32,
            cost_usd: Some(15.5),
            remaining_usd: Some(34.5),
            limit_usd: Some(50.0),
            limit_label: Some("cc-switch".to_string()),
        })
    );
}

#[test]
fn applies_live_app_server_event_names() {
    let mut snapshot = blank_snapshot();

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/started",
            "params": {
                "thread": {
                    "id": "thr_live",
                    "title": "live thread",
                    "model": "gpt-5.4",
                    "modelProvider": "openai"
                }
            }
        })
    ));
    assert_eq!(snapshot.thread_id.as_deref(), Some("thr_live"));
    assert_eq!(snapshot.thread_name.as_deref(), Some("live thread"));
    assert_eq!(snapshot.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(snapshot.model_provider.as_deref(), Some("openai"));

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "id": 3,
            "method": "turn/start",
            "params": {
                "threadId": "thr_live",
                "input": [
                    {
                        "type": "text",
                        "text": "inspect runtime"
                    },
                    {
                        "type": "skill",
                        "name": "filesystem.read",
                        "path": "/tmp/input"
                    },
                    {
                        "type": "skill",
                        "name": "shell.exec",
                        "path": "/tmp/input"
                    }
                ]
            }
        })
    ));
    assert_eq!(snapshot.skill_count, 2);

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/status/changed",
            "params": {
                "threadId": "thr_live",
                "status": "busy"
            }
        })
    ));
    assert_eq!(snapshot.turn_status.as_deref(), Some("busy"));

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "thr_live",
                "tokenUsage": {
                    "used": 1200,
                    "limit": 64000
                }
            }
        })
    ));
    assert_eq!(
        snapshot.token_usage,
        Some(TokenUsage {
            used: 1200,
            limit: 1050000,
        })
    );

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "turn/plan/updated",
            "params": {
                "plan": [
                    { "step": "inspect runtime", "status": "completed" },
                    { "step": "wire HUD", "status": "inProgress" }
                ]
            }
        })
    ));
    assert_eq!(
        snapshot.plan.as_deref(),
        Some("inspect runtime: completed; wire HUD: inProgress")
    );

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "account/rateLimits/updated",
            "params": {
                "rateLimits": {
                    "limitId": "codex",
                    "limitName": "Codex",
                    "primary": {
                        "usedPercent": 31
                    }
                }
            }
        })
    ));
    assert_eq!(
        snapshot.rate_limit,
        Some(RateLimitSummary {
            used_percent: 31,
            cost_usd: None,
            remaining_usd: None,
            limit_usd: None,
            limit_label: Some("Codex".to_string()),
        })
    );

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "turn/completed",
            "params": {
                "threadId": "thr_live",
                "status": "done"
            }
        })
    ));
    assert_eq!(snapshot.turn_status.as_deref(), Some("done"));

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "item/completed",
            "params": {
                "threadId": "thr_live",
                "type": "mcpToolCall",
                "name": "list_files",
                "status": "completed"
            }
        })
    ));
    assert_eq!(
        snapshot.tool_summary.as_deref(),
        Some("mcpToolCall list_files completed")
    );
}

#[test]
fn applies_codex_0135_token_usage_shape_for_context_display() {
    let mut snapshot = blank_snapshot();

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/started",
            "params": {
                "thread": {
                    "id": "019e7ff5-c908-7020-8d68-b7bd2858af24"
                }
            }
        })
    ));
    assert_eq!(
        snapshot.thread_id.as_deref(),
        Some("019e7ff5-c908-7020-8d68-b7bd2858af24")
    );

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "id": 1,
            "result": {
                "thread": {
                    "id": "019e7ff5-c908-7020-8d68-b7bd2858af24",
                    "name": "HUD repair",
                    "status": { "type": "active" },
                    "cwd": "/Users/me/codex-hud",
                    "gitInfo": {
                        "branch": "main"
                    }
                }
            }
        })
    ));
    assert_eq!(snapshot.thread_name.as_deref(), Some("HUD repair"));

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "019e7ff5-c908-7020-8d68-b7bd2858af24",
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
    assert_eq!(
        snapshot.token_usage,
        Some(TokenUsage {
            used: 9216,
            limit: 128000,
        })
    );
}

#[test]
fn uses_known_model_context_window_when_payload_limit_is_stale() {
    let mut snapshot = blank_snapshot();

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/started",
            "params": {
                "thread": {
                    "id": "thr_gpt55",
                    "model": "gpt-5.5"
                }
            }
        })
    ));
    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "thr_gpt55",
                "tokenUsage": {
                    "total": {
                        "totalTokens": 130000
                    },
                    "modelContextWindow": 128000
                }
            }
        })
    ));

    assert_eq!(
        snapshot.token_usage,
        Some(TokenUsage {
            used: 130000,
            limit: 1050000,
        })
    );
}

#[test]
fn does_not_treat_scalar_total_as_context_limit() {
    let mut snapshot = blank_snapshot();

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/started",
            "params": {
                "thread": {
                    "id": "thr_total",
                    "model": "gpt-5.4-mini"
                }
            }
        })
    ));
    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "thr_total",
                "tokenUsage": {
                    "used": 27333,
                    "total": 27333
                }
            }
        })
    ));

    assert_eq!(
        snapshot.token_usage,
        Some(TokenUsage {
            used: 27333,
            limit: 400000,
        })
    );
}

#[test]
fn prefers_live_context_tokens_over_cumulative_thread_total() {
    let mut snapshot = blank_snapshot();

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/started",
            "params": {
                "thread": {
                    "id": "thr_live_context",
                    "model": "gpt-5.4"
                }
            }
        })
    ));
    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "thr_live_context",
                "tokenUsage": {
                    "total": {
                        "totalTokens": 990000
                    },
                    "context": {
                        "totalTokens": 42000
                    },
                    "modelContextWindow": 1050000
                }
            }
        })
    ));

    assert_eq!(
        snapshot.token_usage,
        Some(TokenUsage {
            used: 42000,
            limit: 1050000,
        })
    );
}

#[test]
fn clears_thread_usage_when_binding_new_thread() {
    let mut snapshot = blank_snapshot();

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/started",
            "params": {
                "thread": {
                    "id": "thr_old",
                    "model": "gpt-5.4",
                    "tokenUsage": {
                        "total": {
                            "totalTokens": 900000
                        },
                        "modelContextWindow": 1050000
                    }
                }
            }
        })
    ));
    assert_eq!(
        snapshot.token_usage,
        Some(TokenUsage {
            used: 900000,
            limit: 1050000,
        })
    );

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/started",
            "params": {
                "thread": {
                    "id": "thr_new",
                    "model": "gpt-5.4"
                }
            }
        })
    ));

    assert_eq!(snapshot.thread_id.as_deref(), Some("thr_new"));
    assert_eq!(snapshot.token_usage, None);
}

#[test]
fn result_thread_can_rebind_after_stale_thread_update() {
    let mut snapshot = blank_snapshot();

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "method": "thread/updated",
            "params": {
                "threadId": "thr_stale",
                "name": "old terminal",
                "tokenUsage": {
                    "total": {
                        "totalTokens": 900000
                    },
                    "modelContextWindow": 1050000
                }
            }
        })
    ));
    assert_eq!(snapshot.thread_id.as_deref(), Some("thr_stale"));

    assert!(apply_app_server_message(
        &mut snapshot,
        &json!({
            "id": 1,
            "result": {
                "thread": {
                    "id": "thr_current",
                    "name": "current terminal",
                    "model": "gpt-5.4"
                }
            }
        })
    ));

    assert_eq!(snapshot.thread_id.as_deref(), Some("thr_current"));
    assert_eq!(snapshot.thread_name.as_deref(), Some("current terminal"));
    assert_eq!(snapshot.token_usage, None);
}
