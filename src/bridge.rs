use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::protocol::{connect_unix_websocket, ProtocolError, ProtocolResult};

pub struct LocalWsBridgeHandle {
    addr: SocketAddr,
    shutdown: oneshot::Sender<()>,
    join: JoinHandle<ProtocolResult<()>>,
}

impl LocalWsBridgeHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn local_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    pub async fn shutdown(self) -> ProtocolResult<()> {
        let _ = self.shutdown.send(());
        match self.join.await {
            Ok(result) => result,
            Err(err) => Err(ProtocolError::TaskJoin(err.to_string())),
        }
    }
}

pub async fn spawn_local_ws_bridge(backend_socket: &Path) -> ProtocolResult<LocalWsBridgeHandle> {
    spawn_local_ws_bridge_with_observer(backend_socket, None).await
}

pub async fn spawn_local_ws_bridge_with_observer(
    backend_socket: &Path,
    observer: Option<Sender<serde_json::Value>>,
) -> ProtocolResult<LocalWsBridgeHandle> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let backend_socket = backend_socket.to_path_buf();
    let (shutdown, shutdown_rx) = oneshot::channel();
    let join = tokio::spawn(run_bridge(listener, backend_socket, shutdown_rx, observer));

    Ok(LocalWsBridgeHandle {
        addr,
        shutdown,
        join,
    })
}

async fn run_bridge(
    listener: TcpListener,
    backend_socket: PathBuf,
    mut shutdown: oneshot::Receiver<()>,
    observer: Option<Sender<serde_json::Value>>,
) -> ProtocolResult<()> {
    let mut relay_tasks = JoinSet::new();

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                relay_tasks.abort_all();
                while relay_tasks.join_next().await.is_some() {}
                return Ok(());
            }
            accepted = listener.accept() => {
                let (client, _) = accepted?;
                let backend_socket = backend_socket.clone();
                let observer = observer.clone();
                relay_tasks.spawn(async move {
                    let _ = relay_connection(client, backend_socket, observer).await;
                });
            }
        }
    }
}

async fn relay_connection(
    client: TcpStream,
    backend_socket: PathBuf,
    observer: Option<Sender<serde_json::Value>>,
) -> ProtocolResult<()> {
    let client_ws = accept_async(client).await?;
    let backend_ws = connect_unix_websocket(&backend_socket).await?;

    let (client_write, client_read) = client_ws.split();
    let (backend_write, backend_read) = backend_ws.split();

    tokio::select! {
        result = pump(client_read, backend_write, observer.clone()) => result,
        result = pump(backend_read, client_write, observer) => result,
    }
}

async fn pump<R, W>(
    mut reader: R,
    mut writer: W,
    observer: Option<Sender<serde_json::Value>>,
) -> ProtocolResult<()>
where
    R: Stream<Item = Result<Message, WsError>> + Unpin,
    W: Sink<Message, Error = WsError> + Unpin,
{
    while let Some(message) = reader.next().await {
        let message = message?;
        if let Some(observer) = observer.as_ref() {
            if let Some(json) = message_to_json(&message) {
                let _ = observer.send(json);
            }
        }
        let is_close = message.is_close();
        writer.send(message).await?;
        if is_close {
            break;
        }
    }

    Ok(())
}

fn message_to_json(message: &Message) -> Option<serde_json::Value> {
    match message {
        Message::Text(text) => serde_json::from_str(text).ok(),
        Message::Binary(bytes) => serde_json::from_slice(bytes).ok(),
        _ => None,
    }
}
