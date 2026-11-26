use crate::ids::{ProjectId, SessionId};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Proyek Git yang terdaftar di daemon Latch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub repo_path: String,
    pub default_branch: String,
    pub created_at_ms: i64,
}

/// Mode injeksi proxy LLM untuk sesi agen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    Injected,
    Bypass,
}

impl ProxyMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Injected => "injected",
            Self::Bypass => "bypass",
        }
    }
}

impl fmt::Display for ProxyMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error)]
#[error("invalid proxy mode: {0}")]
pub struct ParseProxyModeError(String);

impl FromStr for ProxyMode {
    type Err = ParseProxyModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "injected" => Ok(Self::Injected),
            "bypass" => Ok(Self::Bypass),
            other => Err(ParseProxyModeError(other.to_string())),
        }
    }
}

/// Status siklus hidup sesi agen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Paused,
    WaitingApproval,
    Hibernated,
    Completed,
    Tripped,
    Failed,
}

impl SessionStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::WaitingApproval => "waiting_approval",
            Self::Hibernated => "hibernated",
            Self::Completed => "completed",
            Self::Tripped => "tripped",
            Self::Failed => "failed",
        }
    }

    /// Mengembalikan `true` untuk status akhir sesi.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Tripped | Self::Failed)
    }
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Error)]
#[error("invalid session status: {0}")]
pub struct ParseSessionStatusError(String);

impl FromStr for SessionStatus {
    type Err = ParseSessionStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "running" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "waiting_approval" => Ok(Self::WaitingApproval),
            "hibernated" => Ok(Self::Hibernated),
            "completed" => Ok(Self::Completed),
            "tripped" => Ok(Self::Tripped),
            "failed" => Ok(Self::Failed),
            other => Err(ParseSessionStatusError(other.to_string())),
        }
    }
}

/// Entitas sesi PTY agen yang terikat pada satu git worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub agent_profile: String,
    pub worktree_path: String,
    pub branch_name: String,
    pub status: SessionStatus,
    pub pid: Option<u32>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub task: Option<String>,
    pub exit_code: Option<i32>,
    pub trip_reason: Option<String>,
    pub proxy_mode: ProxyMode,
    pub sandbox_provider: String,
    pub cleaned_at_ms: Option<i64>,
}

/// Permintaan persetujuan aksi berisiko (Fase 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    pub session_id: SessionId,
    pub action_type: String,
    pub payload_summary: String,
    pub decision: String,
    pub decided_by: Option<String>,
    pub decided_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>,
}

/// Eksekusi pipeline DAG (Fase 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineRun {
    pub id: String,
    pub project_id: ProjectId,
    pub workflow_name: String,
    pub dag_state_json: String,
    pub status: String,
    pub created_at_ms: i64,
    pub updated_at_ms: Option<i64>,
    pub task: Option<String>,
    pub total_cost_usd: f64,
}

/// Catatan telemetri proxy LLM (Fase 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryRecord {
    pub id: i64,
    pub session_id: SessionId,
    pub provider: String,
    pub model: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub estimated_cost_usd: f64,
    pub http_status: Option<i32>,
    pub latency_ms: Option<i64>,
    pub ttfb_ms: Option<i64>,
    pub usage_source: String,
    pub error_kind: Option<String>,
    pub request_fingerprint: Option<String>,
    pub timestamp_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_status_serde_snake_case() {
        let json = serde_json::to_string(&SessionStatus::WaitingApproval).unwrap();
        assert_eq!(json, "\"waiting_approval\"");
        let parsed: SessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SessionStatus::WaitingApproval);
    }
}
