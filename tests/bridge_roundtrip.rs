use std::path::Path;

use codex_hud::bridge::spawn_local_ws_bridge;
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
                "id": 7,
                "method": "thread/read",
                "params": { "threadId": "thr_1" }
            })
            .to_string(),
        ))
        .await
        .unwrap();

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
