use crate::model::{Session, SessionStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Write};
use thiserror::Error;

/// Jenis frame pada protokol UDS Latch.
pub const FRAME_RPC: u8 = 0x00;
pub const FRAME_PTY_OUTPUT: u8 = 0x01;
pub const FRAME_PTY_SNAPSHOT: u8 = 0x02;
pub const FRAME_PTY_DROPPED: u8 = 0x03;

/// Batas ukuran frame JSON-RPC (1 MiB) dan frame biner PTY (4 MiB).
pub const MAX_RPC_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_BINARY_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Kode galat protokol sesuai `docs/API.md`.
pub mod err_code {
    pub const SESSION_NOT_FOUND: &str = "session_not_found";
    pub const PROJECT_NOT_FOUND: &str = "project_not_found";
    pub const INVALID_STATE_TRANSITION: &str = "invalid_state_transition";
    pub const MERGE_CONFLICT: &str = "merge_conflict";
    pub const PROFILE_INVALID: &str = "profile_invalid";
    pub const INVALID_REQUEST: &str = "invalid_request";
    pub const INTERNAL: &str = "internal";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    #[must_use]
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    #[must_use]
    pub fn err(
        id: Value,
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code: code.into(),
                message: message.into(),
                details,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

/// Event siklus hidup sesi yang dipublikasikan daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    #[serde(rename = "session.created")]
    SessionCreated { session: Box<Session> },
    #[serde(rename = "session.status")]
    SessionStatus {
        session_id: String,
        from: SessionStatus,
        to: SessionStatus,
        reason: Option<String>,
    },
}

/// Frame yang didekode dari jalur UDS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("empty frame")]
    Empty,

    #[error("frame too large: {len} bytes (max {max})")]
    TooLarge { len: usize, max: usize },
}

/// Menulis satu frame berprefix panjang 4-byte big-endian dengan 1-byte jenis frame.
pub fn write_frame<W: Write>(writer: &mut W, kind: u8, data: &[u8]) -> Result<(), FrameError> {
    let total_len = data.len().saturating_add(1);
    let max = if kind == FRAME_RPC {
        MAX_RPC_FRAME_BYTES
    } else {
        MAX_BINARY_FRAME_BYTES
    };
    if total_len > max {
        return Err(FrameError::TooLarge {
            len: total_len,
            max,
        });
    }
    let len_u32 = u32::try_from(total_len).map_err(|_| FrameError::TooLarge {
        len: total_len,
        max,
    })?;
    writer.write_all(&len_u32.to_be_bytes())?;
    writer.write_all(&[kind])?;
    writer.write_all(data)?;
    writer.flush()?;
    Ok(())
}

/// Membaca satu frame berprefix panjang 4-byte big-endian.
pub fn read_frame<R: Read>(reader: &mut R) -> Result<Frame, FrameError> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let total_len = u32::from_be_bytes(len_buf) as usize;
    if total_len == 0 {
        return Err(FrameError::Empty);
    }
    if total_len > MAX_BINARY_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            len: total_len,
            max: MAX_BINARY_FRAME_BYTES,
        });
    }
    let mut buf = vec![0u8; total_len];
    reader.read_exact(&mut buf)?;
    let kind = buf[0];
    if kind == FRAME_RPC && total_len > MAX_RPC_FRAME_BYTES {
        return Err(FrameError::TooLarge {
            len: total_len,
            max: MAX_RPC_FRAME_BYTES,
        });
    }
    let data = buf.split_off(1);
    Ok(Frame { kind, data })
}

/// Mengodekan jumlah byte terbuang untuk frame `FRAME_PTY_DROPPED`.
#[must_use]
pub fn encode_dropped(bytes: u64) -> [u8; 8] {
    bytes.to_be_bytes()
}

/// Mendekode jumlah byte terbuang dari payload `FRAME_PTY_DROPPED`.
#[must_use]
pub fn decode_dropped(data: &[u8]) -> Option<u64> {
    let arr: [u8; 8] = data.try_into().ok()?;
    Some(u64::from_be_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_rpc_and_binary_frames() {
        let mut buf = Vec::new();
        write_frame(&mut buf, FRAME_RPC, br#"{"ok":true}"#).unwrap();
        write_frame(&mut buf, FRAME_PTY_DROPPED, &encode_dropped(42)).unwrap();

        let mut cursor = Cursor::new(buf);
        let f1 = read_frame(&mut cursor).unwrap();
        assert_eq!(f1.kind, FRAME_RPC);
        assert_eq!(f1.data, br#"{"ok":true}"#);

        let f2 = read_frame(&mut cursor).unwrap();
        assert_eq!(f2.kind, FRAME_PTY_DROPPED);
        assert_eq!(decode_dropped(&f2.data), Some(42));
    }
}
