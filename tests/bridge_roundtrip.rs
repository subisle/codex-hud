use std::path::Path;
use std::sync::mpsc;

use codex_hud::bridge::{spawn_local_ws_bridge, spawn_local_ws_bridge_with_observer};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tempfile::tempdir;
use tokio::net::UnixListener;
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Message};

#[tokio::test]
async fn local_ws_bridge_relays_frames_to_the_unix_backend_and_back() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("backend.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();

    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        let client_notification = ws.next().await.unwrap().unwrap();
        let text = client_notification.into_text().unwrap();
        assert_eq!(
            text,
            json!({
                "method": "thread/ping",
                "params": { "source": "client" }
            })
            .to_string()
        );

        ws.send(Message::Text(
            json!({
                "method": "thread/updated",
                "params": { "source": "backend" }
            })
            .to_string(),
        ))
        .await
        .unwrap();

        let msg = ws.next().await.unwrap().unwrap();
        let text = msg.into_text().unwrap();
        assert_eq!(
            text,
            json!({"id": 7, "method": "thread/read", "params": {"threadId": "thr_1"}}).to_string()
        );

        ws.send(Message::Text(
            json!({
                "id": 7,
                "result": { "thread": { "id": "thr_1" } }
            })
            .to_string(),
        ))
        .await
        .unwrap();

        text
    });

    let bridge = spawn_local_ws_bridge(Path::new(&socket_path))
        .await
        .unwrap();
    let url = bridge.local_url();

    let (mut client, _) = connect_async(&url).await.unwrap();
    client
        .send(Message::Text(
            json!({
                "method": "thread/ping",
                "params": { "source": "client" }
            })
            .to_string(),
        ))
        .await
        .unwrap();
    client
        .send(Message::Text(
            json!({
                "id": 7,
                "method": "thread/read",
                "params": { "threadId": "thr_1" }
            })
            .to_string(),
        ))
        .await
        .unwrap();

    let notification = client.next().await.unwrap().unwrap().into_text().unwrap();
    assert_eq!(
        notification,
        json!({
            "method": "thread/updated",
            "params": { "source": "backend" }
        })
        .to_string()
    );

    let response = client.next().await.unwrap().unwrap().into_text().unwrap();
    assert_eq!(
        response,
        json!({
            "id": 7,
            "result": { "thread": { "id": "thr_1" } }
        })
        .to_string()
    );

    assert_eq!(
        backend.await.unwrap(),
        json!({
            "id": 7,
            "method": "thread/read",
            "params": { "threadId": "thr_1" }
        })
        .to_string()
    );

    bridge.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_ws_bridge_observer_receives_client_and_backend_json_frames() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("backend.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let (observed_tx, observed_rx) = mpsc::channel();

    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        let request = ws.next().await.unwrap().unwrap();
        assert_eq!(
            request.into_text().unwrap(),
            json!({
                "id": 1,
                "method": "thread/start",
                "params": { "model": "gpt-5.4" }
            })
            .to_string()
        );

        ws.send(Message::Text(
            json!({
                "id": 1,
                "result": {
                    "thread": {
                        "id": "thr_observed",
                        "title": "observed thread"
                    }
                }
            })
            .to_string(),
        ))
        .await
        .unwrap();
    });

    let bridge = spawn_local_ws_bridge_with_observer(Path::new(&socket_path), Some(observed_tx))
        .await
        .unwrap();
    let (mut client, _) = connect_async(bridge.local_url()).await.unwrap();

    client
        .send(Message::Text(
            json!({
                "id": 1,
                "method": "thread/start",
                "params": { "model": "gpt-5.4" }
            })
            .to_string(),
        ))
        .await
        .unwrap();

    let response = client.next().await.unwrap().unwrap().into_text().unwrap();
    assert_eq!(
        response,
        json!({
            "id": 1,
            "result": {
                "thread": {
                    "id": "thr_observed",
                    "title": "observed thread"
                }
            }
        })
        .to_string()
    );

    let first = observed_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let second = observed_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    assert_eq!(first["method"], "thread/start");
    assert_eq!(second["result"]["thread"]["id"], "thr_observed");

    backend.await.unwrap();
    bridge.shutdown().await.unwrap();
}
