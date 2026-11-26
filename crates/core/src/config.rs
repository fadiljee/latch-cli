use crate::model::ProxyMode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Kumpulan jalur direktori dan berkas konfigurasi/state Latch.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub socket_path: PathBuf,
}

impl Paths {
    /// Menyelesaikan direktori Latch dari `LATCH_CONFIG_DIR`, `LATCH_STATE_DIR`, dan variabel XDG.
    pub fn resolve() -> Result<Self, ConfigError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);

        let config_dir = match std::env::var_os("LATCH_CONFIG_DIR") {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => match std::env::var_os("XDG_CONFIG_HOME") {
                Some(v) if !v.is_empty() => PathBuf::from(v).join("latch"),
                _ => home
                    .as_ref()
                    .ok_or(ConfigError::MissingHome)?
                    .join(".config")
                    .join("latch"),
            },
        };

        let state_override = std::env::var_os("LATCH_STATE_DIR").filter(|v| !v.is_empty());
        let state_dir = match &state_override {
            Some(v) => PathBuf::from(v),
            None => match std::env::var_os("XDG_STATE_HOME") {
                Some(v) if !v.is_empty() => PathBuf::from(v).join("latch"),
                _ => home
                    .as_ref()
                    .ok_or(ConfigError::MissingHome)?
                    .join(".local")
                    .join("state")
                    .join("latch"),
            },
        };

        // Jika `LATCH_STATE_DIR` disetel (misalnya di tes), socket diletakkan di dalamnya agar terisolasi.
        let socket_path = if state_override.is_some() {
            state_dir.join("daemon.sock")
        } else if let Some(runtime_dir) =
            std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty())
        {
            PathBuf::from(runtime_dir).join("latch.sock")
        } else {
            state_dir.join("daemon.sock")
        };

        Ok(Self {
            config_dir,
            state_dir,
            socket_path,
        })
    }

    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    #[must_use]
    pub fn agents_file(&self) -> PathBuf {
        self.config_dir.join("agents.toml")
    }

    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join("latch.db")
    }

    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.state_dir.join("logs")
    }

    #[must_use]
    pub fn worktrees_dir(&self) -> PathBuf {
        self.state_dir.join("worktrees")
    }
}

/// Konfigurasi global daemon (`config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct GlobalConfig {
    pub tokio_workers: usize,
    pub ring_buffer_bytes: usize,
    pub kill_grace_ms: u64,
    pub default_cols: u16,
    pub default_rows: u16,
    pub log_max_files: usize,
    pub pty_log_max_bytes: u64,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            tokio_workers: 2,
            ring_buffer_bytes: 1_048_576,
            kill_grace_ms: 3_000,
            default_cols: 120,
            default_rows: 40,
            log_max_files: 5,
            pty_log_max_bytes: 10_485_760,
        }
    }
}

/// Profil CLI agen di `agents.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentProfile {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub pass_env: Vec<String>,
    #[serde(default)]
    pub proxy_mode: ProxyMode,
    #[serde(default)]
    pub proxy_provider: Option<String>,
    #[serde(default)]
    pub resume_args: Vec<String>,
    #[serde(default)]
    pub extra_rw: Vec<String>,
    #[serde(default)]
    pub extra_ro: Vec<String>,
}

/// Isi berkas `agents.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentsFile {
    #[serde(default)]
    pub profiles: BTreeMap<String, AgentProfile>,
}

/// Galat pembacaan atau validasi berkas konfigurasi.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("HOME environment variable is not set")]
    MissingHome,

    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("{path}:{line}:{col}: {message}")]
    Parse {
        path: PathBuf,
        line: usize,
        col: usize,
        message: String,
    },

    #[error("{path}: profile '{profile}' field '{field}' is invalid: {reason}")]
    Validation {
        path: PathBuf,
        profile: String,
        field: String,
        reason: String,
    },
}

/// Memuat `config.toml`; mengembalikan nilai default jika berkas belum ada.
pub fn load_global_config(path: &Path) -> Result<GlobalConfig, ConfigError> {
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let content = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let cfg: GlobalConfig =
        toml::from_str(&content).map_err(|err| toml_err(path, &content, &err))?;
    if cfg.tokio_workers == 0 {
        return Err(ConfigError::Validation {
            path: path.to_path_buf(),
            profile: "<global>".to_string(),
            field: "tokio_workers".to_string(),
            reason: "must be >= 1".to_string(),
        });
    }
    Ok(cfg)
}

/// Memuat dan memvalidasi `agents.toml`; mengembalikan daftar kosong jika berkas belum ada.
pub fn load_agents(path: &Path) -> Result<AgentsFile, ConfigError> {
    if !path.exists() {
        return Ok(AgentsFile::default());
    }
    let content = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let agents: AgentsFile =
        toml::from_str(&content).map_err(|err| toml_err(path, &content, &err))?;

    for (name, profile) in &agents.profiles {
        if !is_valid_identifier(name) {
            return Err(ConfigError::Validation {
                path: path.to_path_buf(),
                profile: name.clone(),
                field: "profiles.<name>".to_string(),
                reason: "profile name must match [A-Za-z0-9._-]+".to_string(),
            });
        }
        if profile.command.trim().is_empty() {
            return Err(ConfigError::Validation {
                path: path.to_path_buf(),
                profile: name.clone(),
                field: "command".to_string(),
                reason: "command must not be empty".to_string(),
            });
        }
    }

    Ok(agents)
}

/// Memuat `agents.toml` yang digabung dengan profil bawaan untuk semua agen populer.
/// Profil yang didefinisikan pengguna di `agents.toml` akan menimpa profil bawaan dengan nama sama.
pub fn load_effective_agents(path: &Path) -> Result<AgentsFile, ConfigError> {
    let mut profiles = builtin_profiles();
    let user = load_agents(path)?;
    for (name, profile) in user.profiles {
        profiles.insert(name, profile);
    }
    Ok(AgentsFile { profiles })
}

/// Daftar profil agen bawaan agar `latch-cli` langsung siap pakai tanpa konfigurasi manual.
/// Semua profil mendukung login akun (OAuth/sesi CLI) maupun API key lewat `pass_env`.
#[must_use]
pub fn builtin_profiles() -> BTreeMap<String, AgentProfile> {
    let mut m = BTreeMap::new();

    let mut add = |name: &str, cmd: &str, args: &[&str], pass_env: &[&str], extra_rw: &[&str]| {
        m.insert(
            name.to_string(),
            AgentProfile {
                command: cmd.to_string(),
                args: args.iter().map(|s| (*s).to_string()).collect(),
                env: BTreeMap::new(),
                pass_env: pass_env.iter().map(|s| (*s).to_string()).collect(),
                proxy_mode: ProxyMode::Bypass,
                proxy_provider: None,
                resume_args: Vec::new(),
                extra_rw: extra_rw.iter().map(|s| (*s).to_string()).collect(),
                extra_ro: Vec::new(),
            },
        );
    };

    add(
        "antigravity",
        "agy",
        &[],
        &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        &["~/.gemini"],
    );
    add(
        "claude",
        "claude",
        &[],
        &["ANTHROPIC_API_KEY"],
        &["~/.claude", "~/.claude.json"],
    );
    add("codex", "codex", &[], &["OPENAI_API_KEY"], &["~/.codex"]);
    add(
        "gemini",
        "gemini",
        &[],
        &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        &["~/.gemini"],
    );
    add(
        "opencode",
        "opencode",
        &[],
        &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "OPENROUTER_API_KEY",
        ],
        &["~/.config/opencode", "~/.local/share/opencode"],
    );
    add(
        "aider",
        "aider",
        &[],
        &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "OPENROUTER_API_KEY",
        ],
        &["~/.aider"],
    );
    add("amp", "amp", &[], &["AMP_API_KEY"], &["~/.config/amp"]);
    add(
        "cline",
        "cline",
        &[],
        &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"],
        &["~/.cline"],
    );
    add(
        "copilot",
        "copilot",
        &[],
        &["GITHUB_TOKEN"],
        &["~/.config/github-copilot"],
    );
    add(
        "cursor",
        "cursor-agent",
        &[],
        &["CURSOR_API_KEY"],
        &["~/.cursor"],
    );
    add("droid", "droid", &[], &["FACTORY_API_KEY"], &["~/.factory"]);
    add(
        "goose",
        "goose",
        &["session"],
        &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GOOGLE_API_KEY"],
        &["~/.config/goose"],
    );
    add("kiro", "kiro-cli", &[], &[], &["~/.kiro"]);
    add(
        "qwen",
        "qwen",
        &[],
        &["OPENAI_API_KEY", "DASHSCOPE_API_KEY"],
        &["~/.qwen"],
    );
    add("shell", "sh", &[], &[], &[]);

    m
}

/// Memeriksa apakah nama proyek atau profil hanya mengandung `[A-Za-z0-9._-]+`.
#[must_use]
pub fn is_valid_identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

fn toml_err(path: &Path, content: &str, err: &toml::de::Error) -> ConfigError {
    let (line, col) = err
        .span()
        .map_or((1, 1), |span| byte_offset_to_line_col(content, span.start));
    ConfigError::Parse {
        path: path.to_path_buf(),
        line,
        col,
        message: err.message().to_string(),
    }
}

fn byte_offset_to_line_col(content: &str, offset: usize) -> (usize, usize) {
    let safe_offset = offset.min(content.len());
    let prefix = &content[..safe_offset];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() + 1;
    let col = prefix
        .rsplit('\n')
        .next()
        .map_or(1, |s| s.chars().count() + 1);
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_valid_agents_fixture() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents_valid.toml");
        let agents = load_agents(&fixture).unwrap();
        assert_eq!(agents.profiles.len(), 2);
        assert_eq!(agents.profiles["coder-claude"].command, "claude");
        assert_eq!(agents.profiles["sleeper"].proxy_mode, ProxyMode::Bypass);
    }

    #[test]
    fn reject_unknown_field_with_line_number() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agents_invalid_unknown.toml");
        let err = load_agents(&fixture).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(":4:"), "expected line 4 in error: {msg}");
    }

    #[test]
    fn reject_empty_command_with_field_name() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agents_invalid_empty_cmd.toml");
        let err = load_agents(&fixture).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("command"),
            "expected field name in error: {msg}"
        );
    }
}
