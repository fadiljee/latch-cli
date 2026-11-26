use crate::protocol::{
    FRAME_PTY_DROPPED, FRAME_PTY_OUTPUT, FRAME_PTY_SNAPSHOT, FRAME_RPC, FrameError, RpcError,
    RpcRequest, RpcResponse, decode_dropped, read_frame, write_frame,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use thiserror::Error;

/// Galat klien UDS sinkron.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("failed to connect to daemon socket: {0}")]
    Connect(#[from] io::Error),

    #[error("frame error: {0}")]
    Frame(#[from] FrameError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("daemon error [{code}]: {message}")]
    Rpc {
        code: String,
        message: String,
        details: Option<Value>,
    },

    #[error("unexpected frame kind: {0}")]
    UnexpectedFrame(u8),
}

impl From<RpcError> for ClientError {
    fn from(err: RpcError) -> Self {
        Self::Rpc {
            code: err.code,
            message: err.message,
            details: err.details,
        }
    }
}

/// Klien sinkron berbasis Unix Domain Socket untuk CLI dan TUI.
pub struct Client {
    stream: UnixStream,
    next_id: u64,
}

impl Client {
    pub fn connect(socket_path: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket_path)?;
        Ok(Self { stream, next_id: 1 })
    }

    pub fn call<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<T, ClientError> {
        let id = self.next_id;
        self.next_id += 1;

        let req = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: json!(id),
            method: method.to_string(),
            params,
        };
        let payload = serde_json::to_vec(&req)?;
        write_frame(&mut self.stream, FRAME_RPC, &payload)?;

        let frame = read_frame(&mut self.stream)?;
        if frame.kind != FRAME_RPC {
            return Err(ClientError::UnexpectedFrame(frame.kind));
        }
        let resp: RpcResponse = serde_json::from_slice(&frame.data)?;
        if let Some(err) = resp.error {
            return Err(err.into());
        }
        let result = resp.result.unwrap_or(Value::Null);
        Ok(serde_json::from_value(result)?)
    }

    /// Membuka aliran attach PTY untuk satu sesi, mengkonsumsi koneksi menjadi `AttachStream`.
    pub fn attach(mut self, session_id: &str) -> Result<AttachStream, ClientError> {
        let id = self.next_id;
        self.next_id += 1;

        let req = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: json!(id),
            method: "session.attach".to_string(),
            params: json!({ "id": session_id }),
        };
        let payload = serde_json::to_vec(&req)?;
        write_frame(&mut self.stream, FRAME_RPC, &payload)?;

        let frame = read_frame(&mut self.stream)?;
        if frame.kind != FRAME_RPC {
            return Err(ClientError::UnexpectedFrame(frame.kind));
        }
        let resp: RpcResponse = serde_json::from_slice(&frame.data)?;
        if let Some(err) = resp.error {
            return Err(err.into());
        }

        Ok(AttachStream {
            stream: self.stream,
        })
    }
}

/// Item aliran dari koneksi `session.attach`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachEvent {
    Snapshot(Vec<u8>),
    Output(Vec<u8>),
    Dropped(u64),
}

/// Iterator sinkron untuk membaca frame PTY dari koneksi attach.
pub struct AttachStream {
    stream: UnixStream,
}

impl Iterator for AttachStream {
    type Item = Result<AttachEvent, ClientError>;

    fn next(&mut self) -> Option<Self::Item> {
        match read_frame(&mut self.stream) {
            Ok(frame) => match frame.kind {
                FRAME_PTY_SNAPSHOT => Some(Ok(AttachEvent::Snapshot(frame.data))),
                FRAME_PTY_OUTPUT => Some(Ok(AttachEvent::Output(frame.data))),
                FRAME_PTY_DROPPED => {
                    let bytes = decode_dropped(&frame.data).unwrap_or(0);
                    Some(Ok(AttachEvent::Dropped(bytes)))
                }
                other => Some(Err(ClientError::UnexpectedFrame(other))),
            },
            Err(FrameError::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
                ) =>
            {
                None
            }
            Err(err) => Some(Err(ClientError::Frame(err))),
        }
    }
}
