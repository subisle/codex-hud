use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::path::Path;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::net::UnixStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{client_async, WebSocketStream};

pub type ProtocolResult<T> = Result<T, ProtocolError>;

#[derive(Debug)]
pub enum ProtocolError {
    Io(std::io::Error),
    WebSocket(WsError),
    Json(serde_json::Error),
    Rpc(Value),
    Closed,
    UnexpectedMessage(String),
    TaskJoin(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::WebSocket(err) => write!(f, "WebSocket error: {err}"),
            Self::Json(err) => write!(f, "JSON error: {err}"),
            Self::Rpc(error) => write!(f, "JSON-RPC error: {error}"),
            Self::Closed => write!(f, "transport closed"),
            Self::UnexpectedMessage(message) => write!(f, "unexpected message: {message}"),
            Self::TaskJoin(message) => write!(f, "task join error: {message}"),
        }
    }
}

impl Error for ProtocolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::WebSocket(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::Rpc(_) | Self::Closed | Self::UnexpectedMessage(_) | Self::TaskJoin(_) => None,
        }
    }
}

impl From<std::io::Error> for ProtocolError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<WsError> for ProtocolError {
    fn from(err: WsError) -> Self {
        Self::WebSocket(err)
    }
}

impl From<serde_json::Error> for ProtocolError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub title: Option<String>,
    pub version: String,
}

#[derive(Debug)]
pub struct AppServerClient {
    socket: WebSocketStream<UnixStream>,
    next_id: u64,
    buffered: VecDeque<Value>,
}

impl AppServerClient {
    pub async fn connect_unix(socket_path: &Path) -> ProtocolResult<Self> {
        let socket = connect_unix_websocket(socket_path).await?;
        Ok(Self {
            socket,
            next_id: 0,
            buffered: VecDeque::new(),
        })
    }

    pub async fn initialize(&mut self, client_info: ClientInfo) -> ProtocolResult<Value> {
        self.request(
            "initialize",
            Some(json!({
                "clientInfo": client_info,
                "capabilities": null
            })),
        )
        .await
    }

    pub async fn initialized(&mut self) -> ProtocolResult<()> {
        self.notify("initialized", Some(json!({}))).await
    }

    pub async fn thread_read(
        &mut self,
        thread_id: impl AsRef<str>,
        include_turns: bool,
    ) -> ProtocolResult<Value> {
        self.request(
            "thread/read",
            Some(json!({
                "threadId": thread_id.as_ref(),
                "includeTurns": include_turns
            })),
        )
        .await
    }

    pub async fn account_rate_limits_read(&mut self) -> ProtocolResult<Value> {
        self.request("account/rateLimits/read", None).await
    }

    pub async fn request(&mut self, method: &str, params: Option<Value>) -> ProtocolResult<Value> {
        let id = self.next_request_id();
        let mut request = Map::new();
        request.insert("method".to_string(), Value::String(method.to_string()));
        request.insert("id".to_string(), Value::from(id));
        if let Some(params) = params {
            request.insert("params".to_string(), params);
        }

        self.send_value(Value::Object(request)).await?;

        loop {
            let response = self.read_socket_value().await?;
            if response.get("id") == Some(&Value::from(id)) {
                if let Some(error) = response.get("error") {
                    return Err(ProtocolError::Rpc(error.clone()));
                }

                return Ok(response.get("result").cloned().unwrap_or(Value::Null));
            }

            self.buffered.push_back(response);
        }
    }

    pub async fn notify(&mut self, method: &str, params: Option<Value>) -> ProtocolResult<()> {
        let mut notification = Map::new();
        notification.insert("method".to_string(), Value::String(method.to_string()));
        if let Some(params) = params {
            notification.insert("params".to_string(), params);
        }

        self.send_value(Value::Object(notification)).await
    }

    pub async fn next_message(&mut self) -> ProtocolResult<Value> {
        if let Some(message) = self.buffered.pop_front() {
            return Ok(message);
        }

        self.read_socket_value().await
    }

    async fn send_value(&mut self, value: Value) -> ProtocolResult<()> {
        self.socket
            .send(Message::Text(serde_json::to_string(&value)?))
            .await?;
        Ok(())
    }

    async fn read_socket_value(&mut self) -> ProtocolResult<Value> {
        loop {
            let message = self.socket.next().await.ok_or(ProtocolError::Closed)??;
            match message {
                Message::Text(text) => return Ok(serde_json::from_str(&text)?),
                Message::Binary(bytes) => return Ok(serde_json::from_slice(&bytes)?),
                Message::Close(_) => return Err(ProtocolError::Closed),
                Message::Ping(_) | Message::Pong(_) => continue,
                _ => continue,
            }
        }
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

pub async fn connect_unix_websocket(
    socket_path: &Path,
) -> ProtocolResult<WebSocketStream<UnixStream>> {
    let stream = UnixStream::connect(socket_path).await?;
    let request = "ws://localhost/".into_client_request()?;
    let (socket, _) = client_async(request, stream).await?;
    Ok(socket)
}
