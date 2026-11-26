//! Model domain, parser konfigurasi, trait sandbox, redaksi rahasia, dan protokol IPC bersama.

pub mod client;
pub mod config;
pub mod ids;
pub mod model;
pub mod protocol;
pub mod redact;
pub mod sandbox;
pub mod transition;

pub use ids::{ProjectId, SessionId};
pub use model::{
    ApprovalRequest, PipelineRun, Project, ProxyMode, Session, SessionStatus, TelemetryRecord,
};
pub use sandbox::{SandboxCapabilities, SandboxError, SandboxPolicy, SandboxProvider, SpawnSpec};
pub use transition::can_transition;
