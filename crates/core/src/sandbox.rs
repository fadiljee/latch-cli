use std::path::PathBuf;
use thiserror::Error;

/// Spesifikasi eksekusi proses anak PTY.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

/// Kebijakan isolasi filesystem dan jaringan untuk proses sesi.
#[derive(Debug, Clone, Default)]
pub struct SandboxPolicy {
    pub rw_paths: Vec<PathBuf>,
    pub ro_paths: Vec<PathBuf>,
    pub deny_hints: Vec<PathBuf>,
    pub allow_network: Vec<String>,
}

/// Kemampuan sandbox yang tersedia di sistem host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxCapabilities {
    pub available: bool,
    pub abi: Option<u32>,
    pub detail: String,
}

/// Galat pada penyedia sandbox.
#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("sandbox unavailable: {0}")]
    Unavailable(String),

    #[error("invalid sandbox policy: {0}")]
    InvalidPolicy(String),
}

/// Abstraksi penyedia sandbox agar implementasi OS dapat ditukar dan diuji.
pub trait SandboxProvider: Send + Sync {
    /// Nama penyedia sandbox, misalnya `"landlock"` atau `"noop"`.
    fn name(&self) -> &'static str;

    /// Kemampuan sandbox di host saat ini.
    fn capabilities(&self) -> SandboxCapabilities;

    /// Mengubah `SpawnSpec` agar kebijakan diterapkan di proses anak sebelum `exec`.
    fn wrap_command(
        &self,
        policy: &SandboxPolicy,
        spec: &mut SpawnSpec,
    ) -> Result<(), SandboxError>;
}
