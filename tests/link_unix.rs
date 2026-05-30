use std::path::Path;

use codex_hud::protocol::{AppServerClient, ClientInfo};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tempfile::tempdir;
use tokio::net::UnixListener;
use tokio_tungstenite::{accept_async, tungstenite::Message};

#[tokio::test]
async fn unix_link_handshake_supports_initialize_thread_read_and_rate_limits() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("app-server.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        assert_json_frame(&mut ws, "initialize", Some(0), |msg| {
            assert_eq!(msg["params"]["clientInfo"]["name"], "codex-hud");
            assert!(msg.get("jsonrpc").is_none());
        })
        .await;
        ws.send(Message::Text(
            json!({
                "id": 0,
                "result": {
                    "userAgent": "codex-hud-test",
                    "codexHome": "/tmp/codex-hud",
                    "platformFamily": "unix",
                    "platformOs": "macos"
                }
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

        assert_json_frame(&mut ws, "initialized", None, |msg| {
            assert_eq!(msg["params"], json!({}));
        })
        .await;

        assert_json_frame(&mut ws, "thread/read", Some(1), |msg| {
            assert_eq!(msg["params"]["threadId"], "thr_123");
            assert_eq!(msg["params"]["includeTurns"], false);
        })
        .await;
        ws.send(Message::Text(
            json!({
                "id": 1,
                "result": {
                    "thread": {
                        "id": "thr_123"
                    }
                }
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

        assert_json_frame(&mut ws, "account/rateLimits/read", Some(2), |msg| {
            assert!(msg.get("params").is_none());
        })
        .await;
        ws.send(Message::Text(
            json!({
                "id": 2,
                "result": {
                    "rateLimits": {
                        "limitId": "codex",
                        "limitName": null,
                        "primary": null,
                        "secondary": null,
                        "credits": null,
                        "planType": null,
                        "rateLimitReachedType": null
                    },
                    "rateLimitsByLimitId": null
                }
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    });

    let mut client = AppServerClient::connect_unix(Path::new(&socket_path))
        .await
        .unwrap();

    let init = client
        .initialize(ClientInfo {
            name: "codex-hud".to_string(),
            title: Some("Codex HUD".to_string()),
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .await
        .unwrap();
    assert_eq!(init["userAgent"], "codex-hud-test");

    client.initialized().await.unwrap();

    let thread = client.thread_read("thr_123", false).await.unwrap();
    assert_eq!(thread["thread"]["id"], "thr_123");

    let rate_limits = client.account_rate_limits_read().await.unwrap();
    assert_eq!(rate_limits["rateLimits"]["limitId"], "codex");

    server.await.unwrap();
}

async fn assert_json_frame<F>(
    ws: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    method: &str,
    id: Option<u64>,
    check: F,
) where
    F: FnOnce(serde_json::Value),
{
    let msg = ws.next().await.unwrap().unwrap();
    let text = msg.into_text().unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["method"], method);
    match id {
        Some(id) => assert_eq!(value["id"], id),
        None => assert!(value.get("id").is_none()),
    }
    check(value);
}
