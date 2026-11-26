#![allow(clippy::print_stdout)]

use crate::ui;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use latch_core::config::Paths;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Kredensial tersimpan secara lokal di `~/.local/state/latch/auth.json` (izin `0600`).
/// Tidak pernah ditulis ke SQLite atau log.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthStore {
    #[serde(default)]
    pub entries: BTreeMap<String, AuthEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthEntry {
    pub kind: String, // "oauth" | "api_key"
    pub token: String,
    #[serde(default)]
    pub base_url: Option<String>,
}

impl AuthStore {
    #[must_use]
    pub fn file_path(paths: &Paths) -> PathBuf {
        paths.state_dir.join("auth.json")
    }

    pub fn load(paths: &Paths) -> Self {
        let path = Self::file_path(paths);
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&paths.state_dir)?;
        let path = Self::file_path(paths);
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    pub fn set(&mut self, provider: &str, kind: &str, token: String, base_url: Option<String>) {
        self.entries.insert(
            provider.to_string(),
            AuthEntry {
                kind: kind.to_string(),
                token,
                base_url,
            },
        );
    }

    pub fn remove(&mut self, provider: &str) -> bool {
        self.entries.remove(provider).is_some()
    }
}

/// Login OAuth PKCE (S256) langsung ke OpenRouter lewat browser + callback localhost.
/// Satu akun OpenRouter memberi akses ke Claude, GPT, Gemini, DeepSeek, Qwen, dll. tanpa copy-paste API key.
pub fn login_openrouter_pkce(paths: &Paths) -> anyhow::Result<()> {
    let verifier = generate_pkce_verifier()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let callback_url = format!("http://127.0.0.1:{port}/callback");
    let auth_url = format!(
        "https://openrouter.ai/auth?callback_url={}&code_challenge={}&code_challenge_method=S256",
        percent_encode(&callback_url),
        challenge
    );

    ui::print_info("Opening OpenRouter OAuth login in your browser...");
    println!("  {auth_url}");
    open_browser(&auth_url);
    ui::print_info("Waiting for browser callback on 127.0.0.1 (press Ctrl+C to cancel)...");

    let code = wait_for_oauth_code(&listener)?;
    ui::print_info("Exchanging authorization code...");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let key: String = rt.block_on(async {
        let client = reqwest::Client::builder()
            .user_agent(concat!("latch-cli/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let resp = client
            .post("https://openrouter.ai/api/v1/auth/keys")
            .json(&json!({
                "code": code,
                "code_verifier": verifier,
                "code_challenge_method": "S256",
            }))
            .send()
            .await?;
        let status = resp.status();
        let body: Value = resp.json().await?;
        if !status.is_success() {
            anyhow::bail!("OpenRouter token exchange failed ({status}): {body}");
        }
        body["key"]
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| anyhow::anyhow!("missing `key` in OpenRouter response"))
    })?;

    let mut store = AuthStore::load(paths);
    store.set(
        "openrouter",
        "oauth",
        key,
        Some("https://openrouter.ai/api/v1".to_string()),
    );
    store.save(paths)?;
    ui::print_ok(
        "Signed in to OpenRouter via OAuth (saved with 0600 permissions). You can now use built-in `latch` agent!",
    );
    Ok(())
}

/// Login akun GitHub (untuk GitHub Models: GPT-4o, Claude, Llama, DeepSeek) via `gh` OAuth session.
pub fn login_github_models(paths: &Paths) -> anyhow::Result<()> {
    // Coba ambil token OAuth yang sudah aktif di GitHub CLI (`gh`).
    let mut token = read_gh_token();
    if token.is_none() && crate::command_in_path("gh") {
        ui::print_info("Launching GitHub OAuth login via `gh auth login`...");
        let status = Command::new("gh")
            .args(["auth", "login", "--web"])
            .status()?;
        if status.success() {
            token = read_gh_token();
        }
    }

    let tok = token.ok_or_else(|| {
        anyhow::anyhow!(
            "Could not obtain GitHub OAuth token. Install GitHub CLI (`gh`) and run `gh auth login`, or use `/login` -> API Key."
        )
    })?;

    let mut store = AuthStore::load(paths);
    store.set(
        "github",
        "oauth",
        tok,
        Some("https://models.inference.ai.azure.com".to_string()),
    );
    store.save(paths)?;
    ui::print_ok(
        "Signed in to GitHub Models via OAuth (saved with 0600 permissions). Switch to `latch` agent to chat!",
    );
    Ok(())
}

/// Input API key tersembunyi (masked `*`) dan simpan lokal berizin `0600`.
pub fn login_api_key_interactive(paths: &Paths) -> anyhow::Result<()> {
    let providers = vec![
        (
            "anthropic".to_string(),
            "Anthropic API key (Claude Sonnet / Opus)".to_string(),
        ),
        (
            "openai".to_string(),
            "OpenAI API key (GPT-4o / o3 / Codex)".to_string(),
        ),
        (
            "gemini".to_string(),
            "Google Gemini API key (Gemini 2.5 Pro / Flash)".to_string(),
        ),
        (
            "openrouter".to_string(),
            "OpenRouter API key (all models)".to_string(),
        ),
    ];

    let Some(idx) = ui::select_menu("Select Provider for API Key", &providers)? else {
        return Ok(());
    };
    let provider = &providers[idx].0;
    let secret = read_masked_input(&format!("Enter {provider} API key"))?;
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        ui::print_warn("Cancelled (empty key).");
        return Ok(());
    }

    let base_url = if provider == "openrouter" {
        Some("https://openrouter.ai/api/v1".to_string())
    } else {
        None
    };

    let mut store = AuthStore::load(paths);
    store.set(provider, "api_key", trimmed.to_string(), base_url);
    store.save(paths)?;
    ui::print_ok(&format!(
        "Saved {provider} API key locally at {} (mode 0600)",
        AuthStore::file_path(paths).display()
    ));
    Ok(())
}

/// Menampilkan status autentikasi tanpa pernah mencetak isi token/key.
pub fn print_auth_status(paths: &Paths) {
    let store = AuthStore::load(paths);
    println!(
        "  Saved Latch Credentials ({}):",
        AuthStore::file_path(paths).display()
    );
    if store.entries.is_empty() {
        ui::print_info("No saved native OAuth/API credentials yet. Run `/login` to sign in.");
    } else {
        for (provider, entry) in &store.entries {
            ui::print_ok(&format!("{provider:<14} ({})", entry.kind));
        }
    }

    let env_keys = [
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("gemini", "GEMINI_API_KEY"),
        ("openrouter", "OPENROUTER_API_KEY"),
        ("github", "GITHUB_TOKEN"),
    ];
    let active_envs: Vec<String> = env_keys
        .iter()
        .filter(|(_, var)| std::env::var(var).is_ok_and(|v| !v.trim().is_empty()))
        .map(|(p, var)| format!("{p} (${var})"))
        .collect();
    if !active_envs.is_empty() {
        ui::print_info(&format!("Active env keys: {}", active_envs.join(", ")));
    }
}

pub fn logout_interactive(paths: &Paths) -> anyhow::Result<()> {
    let mut store = AuthStore::load(paths);
    if store.entries.is_empty() {
        ui::print_info("No saved credentials to remove.");
        return Ok(());
    }
    let items: Vec<(String, String)> = store
        .entries
        .iter()
        .map(|(k, v)| (k.clone(), format!("Remove saved {} credential", v.kind)))
        .collect();
    if let Some(idx) = ui::select_menu("Sign Out / Remove Credential", &items)? {
        let name = items[idx].0.clone();
        store.remove(&name);
        store.save(paths)?;
        ui::print_ok(&format!("Removed saved credentials for '{name}'"));
    }
    Ok(())
}

fn read_gh_token() -> Option<String> {
    let out = Command::new("gh").args(["auth", "token"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let tok = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if tok.is_empty() { None } else { Some(tok) }
}

fn generate_pkce_verifier() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    let mut f = fs::File::open(Path::new("/dev/urandom"))?;
    f.read_exact(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn wait_for_oauth_code(listener: &TcpListener) -> anyhow::Result<String> {
    let (mut stream, _) = listener.accept()?;
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let first_line = req.lines().next().unwrap_or("");
    let path_and_query = first_line.split_whitespace().nth(1).unwrap_or("");
    let query = path_and_query.split_once('?').map_or("", |(_, q)| q);
    let code = query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        if k == "code" && !v.is_empty() {
            Some(v.to_string())
        } else {
            None
        }
    });

    let html = if code.is_some() {
        "<html><body style='font-family:sans-serif;background:#111;color:#fff;padding:40px'><h2>Latch CLI — Signed In</h2><p>You can close this tab and return to your terminal.</p></body></html>"
    } else {
        "<html><body style='font-family:sans-serif;background:#111;color:#fff;padding:40px'><h2>Latch CLI — Login Failed</h2><p>Missing authorization code.</p></body></html>"
    };
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();

    code.ok_or_else(|| anyhow::anyhow!("OAuth callback did not contain a `code` parameter"))
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(not(target_os = "macos"))]
    let _ = Command::new("xdg-open").arg(url).spawn();
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            other => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

struct SecretRawGuard;

impl Drop for SecretRawGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

fn read_masked_input(prompt: &str) -> io::Result<String> {
    let mut stdout = io::stdout();
    write!(stdout, "  {prompt}: ")?;
    stdout.flush()?;

    enable_raw_mode()?;
    let _guard = SecretRawGuard;

    let mut buf = String::new();
    loop {
        if let Event::Key(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) = event::read()?
        {
            if kind == KeyEventKind::Release {
                continue;
            }
            match (code, modifiers) {
                (KeyCode::Enter, _) => {
                    write!(stdout, "\r\n")?;
                    stdout.flush()?;
                    return Ok(buf);
                }
                (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    write!(stdout, "\r\n")?;
                    stdout.flush()?;
                    return Ok(String::new());
                }
                (KeyCode::Backspace, _) | (KeyCode::Char('\x7f' | '\x08'), _) => {
                    if buf.pop().is_some() {
                        write!(stdout, "\x08 \x08")?;
                        stdout.flush()?;
                    }
                }
                (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                    buf.push(c);
                    write!(stdout, "*")?;
                    stdout.flush()?;
                }
                _ => {}
            }
        }
    }
}
