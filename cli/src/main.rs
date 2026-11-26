#![allow(clippy::print_stdout)]

mod agent;
mod repl;
mod ui;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use clap::{Args, Parser, Subcommand, ValueEnum};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use latch_core::client::{AttachEvent, Client};
use latch_core::config::{Paths, load_agents, load_effective_agents, load_global_config};
use latch_core::model::{Project, Session, SessionStatus};
use latch_daemon::{DaemonConfig, logging};
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(
    name = "latch-cli",
    version,
    about = "Autonomous Rust Agent Runtime CLI"
)]
struct Cli {
    /// Output hasil sebagai JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage the Latch daemon process.
    Daemon {
        #[command(subcommand)]
        action: DaemonCmd,
    },
    /// List available AI coding agents and check which CLIs are installed.
    Agents,
    /// Sign in to an AI coding agent using your subscription account (OAuth).
    Login {
        /// Optional agent profile name; omit to open the interactive OAuth picker.
        agent: Option<String>,
    },
    /// Create a starter ~/.config/latch/agents.toml file for custom profiles.
    Init {
        /// Overwrite existing agents.toml if present.
        #[arg(long)]
        force: bool,
    },
    /// Manage registered Git projects.
    Project {
        #[command(subcommand)]
        action: ProjectCmd,
    },
    /// Launch an agent session in an isolated git worktree (auto-starts daemon).
    Run(RunArgs),
    /// Manage running and completed agent sessions.
    Session {
        #[command(subcommand)]
        action: SessionCmd,
    },
    /// Validate config.toml and agents.toml.
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
}

#[derive(Subcommand)]
enum DaemonCmd {
    Start {
        /// Jalankan di foreground (untuk systemd atau debugging).
        #[arg(long)]
        foreground: bool,
    },
    Stop,
    Status,
}

#[derive(Subcommand)]
enum ProjectCmd {
    Add {
        name: String,
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        default_branch: Option<String>,
    },
    List,
    Remove {
        name: String,
    },
}

#[derive(Args)]
struct RunArgs {
    /// Agent profile name (e.g. claude, codex, gemini, opencode, aider, goose, qwen, shell).
    #[arg(long)]
    profile: String,
    /// Optional task description (also exposed as LATCH_TASK).
    #[arg(long)]
    task: Option<String>,
    /// Registered project name (defaults to current Git repository).
    #[arg(long)]
    project: Option<String>,
    /// Base branch/ref for the new worktree.
    #[arg(long)]
    base: Option<String>,
    /// Attach interactively to the PTY immediately after starting.
    #[arg(long)]
    attach: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum MergeStrategyArg {
    Squash,
    Merge,
}

impl MergeStrategyArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Squash => "squash",
            Self::Merge => "merge",
        }
    }
}

#[derive(Subcommand)]
enum SessionCmd {
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        status: Option<String>,
    },
    Show {
        id: String,
    },
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    Kill {
        id: String,
    },
    Screen {
        id: String,
    },
    Diff {
        id: String,
        #[arg(long)]
        file: Option<String>,
    },
    MergeCheck {
        id: String,
    },
    Merge {
        id: String,
        #[arg(long, value_enum, default_value_t = MergeStrategyArg::Squash)]
        strategy: MergeStrategyArg,
        #[arg(short = 'm', long)]
        message: Option<String>,
    },
    Discard {
        id: String,
    },
    Attach {
        id: String,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    Check,
}

fn main() -> anyhow::Result<()> {
    // Alias argv0 `latchd` menjalankan daemon di foreground secara langsung.
    if let Some(arg0) = std::env::args_os().next()
        && Path::new(&arg0)
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == "latchd")
    {
        return run_daemon_foreground();
    }

    let cli = Cli::parse();
    let Some(command) = cli.command else {
        return repl::run();
    };
    match command {
        Commands::Daemon { action } => match action {
            DaemonCmd::Start { foreground } => {
                if foreground {
                    run_daemon_foreground()
                } else {
                    spawn_daemon_background(false)
                }
            }
            DaemonCmd::Stop => cmd_daemon_stop(cli.json),
            DaemonCmd::Status => cmd_daemon_status(cli.json),
        },
        Commands::Agents => cmd_agents(cli.json),
        Commands::Login { agent } => match agent {
            Some(name) => cmd_login(&name),
            None => cmd_login_interactive(),
        },
        Commands::Init { force } => cmd_init(force),
        Commands::Project { action } => cmd_project(action, cli.json),
        Commands::Run(args) => cmd_run(args, cli.json),
        Commands::Session { action } => cmd_session(action, cli.json),
        Commands::Config { action } => match action {
            ConfigCmd::Check => cmd_config_check(cli.json),
        },
    }
}

fn run_daemon_foreground() -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    let global = load_global_config(&paths.config_file())?;
    let _log_guard = logging::init(&paths, true)?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(global.tokio_workers)
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let cfg = DaemonConfig { paths, global };
        let shutdown = async {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = async {
                    if let Some(ref mut s) = sigterm {
                        s.recv().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {}
            }
        };
        latch_daemon::run(cfg, shutdown).await
    })
}

fn spawn_daemon_background(quiet: bool) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    if paths.socket_path.exists() && UnixStream::connect(&paths.socket_path).is_ok() {
        if !quiet {
            println!("daemon already running at {}", paths.socket_path.display());
        }
        return Ok(());
    }

    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.args(["daemon", "start", "--foreground"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    cmd.spawn()?;

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if paths.socket_path.exists() && UnixStream::connect(&paths.socket_path).is_ok() {
            if !quiet {
                println!("daemon started at {}", paths.socket_path.display());
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!(
        "timed out waiting for daemon socket at {}",
        paths.socket_path.display()
    )
}

pub(crate) fn ensure_daemon_running(paths: &Paths) -> anyhow::Result<()> {
    if paths.socket_path.exists() && UnixStream::connect(&paths.socket_path).is_ok() {
        // Jika binary daemon yang sedang berjalan berbeda/usang (mis. setelah upgrade), restart otomatis.
        if is_daemon_binary_stale(&paths.socket_path) {
            let _ = cmd_daemon_stop_quiet(paths);
        } else {
            return Ok(());
        }
    }
    spawn_daemon_background(true)
}

fn is_daemon_binary_stale(socket_path: &Path) -> bool {
    let Ok(mut client) = Client::connect(socket_path) else {
        return false;
    };
    let Ok(status) = client.call::<Value>("daemon.status", json!({})) else {
        return false;
    };
    let Some(pid) = status["pid"].as_u64() else {
        return false;
    };
    let Ok(running_exe) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
        return false;
    };
    let Ok(current_exe) = std::env::current_exe() else {
        return false;
    };
    running_exe != current_exe
}

fn cmd_daemon_stop_quiet(paths: &Paths) -> anyhow::Result<()> {
    let mut client = Client::connect(&paths.socket_path)?;
    let _: Value = client.call("daemon.shutdown", json!({}))?;
    drop(client);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && paths.socket_path.exists() {
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

pub(crate) fn command_in_path(cmd: &str) -> bool {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(cmd);
        if candidate.is_file() {
            return true;
        }
    }
    false
}

pub(crate) fn cmd_agents(json_out: bool) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    let agents = load_effective_agents(&paths.agents_file())?;
    let rows: Vec<Value> = agents
        .profiles
        .iter()
        .map(|(name, prof)| {
            let installed = command_in_path(&prof.command);
            let env_hint = if prof.pass_env.is_empty() {
                "none".to_string()
            } else {
                prof.pass_env.join(", ")
            };
            json!({
                "name": name,
                "command": prof.command,
                "installed": installed,
                "api_key_env": env_hint,
            })
        })
        .collect();

    if json_out {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        println!(
            "{:<14} {:<12} {:<12} API KEY (OPTIONAL)",
            "PROFILE", "COMMAND", "STATUS"
        );
        for row in rows {
            let status = if row["installed"].as_bool().unwrap_or(false) {
                "installed"
            } else {
                "not found"
            };
            println!(
                "{:<14} {:<12} {:<12} {}",
                row["name"].as_str().unwrap_or(""),
                row["command"].as_str().unwrap_or(""),
                status,
                row["api_key_env"].as_str().unwrap_or("")
            );
        }
        println!(
            "\nTip: sign in with your account first using `latch-cli login <profile>`, or export an optional API key."
        );
    }
    Ok(())
}

pub(crate) fn cmd_login_interactive() -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    let agents = load_effective_agents(&paths.agents_file())?;
    let oauth_providers: &[(&str, &str)] = &[
        (
            "antigravity",
            "Antigravity CLI — Google account OAuth (`agy`)",
        ),
        (
            "claude",
            "Claude Code — Anthropic account OAuth (`claude login`)",
        ),
        (
            "codex",
            "OpenAI Codex — ChatGPT account OAuth (`codex login`)",
        ),
        ("gemini", "Gemini CLI — Google account OAuth (`gemini`)"),
        (
            "opencode",
            "OpenCode — Provider account OAuth (`opencode auth login`)",
        ),
        ("amp", "Amp — Sourcegraph account OAuth (`amp login`)"),
        (
            "copilot",
            "GitHub Copilot CLI — GitHub account OAuth (`copilot`)",
        ),
        (
            "cursor",
            "Cursor Agent — Cursor account OAuth (`cursor-agent login`)",
        ),
        (
            "cline",
            "Cline CLI — Account / provider auth (`cline auth`)",
        ),
        ("droid", "Factory Droid — Account OAuth (`droid`)"),
        (
            "goose",
            "Goose CLI — Interactive account/provider setup (`goose configure`)",
        ),
        (
            "kiro",
            "Kiro CLI — AWS / Builder ID OAuth (`kiro-cli login`)",
        ),
        ("qwen", "Qwen Code — Account OAuth (`qwen`)"),
    ];

    let mut items: Vec<(String, String)> = Vec::new();
    for &(name, desc) in oauth_providers {
        if let Some(prof) = agents.profiles.get(name) {
            let status = if command_in_path(&prof.command) {
                "installed"
            } else {
                "not found"
            };
            items.push((name.to_string(), format!("{desc} [{status}]")));
        }
    }

    if let Some(idx) = ui::select_menu("Choose OAuth Account Login", &items)? {
        let chosen = &items[idx].0;
        return cmd_login(chosen);
    }
    Ok(())
}

pub(crate) fn cmd_login(agent: &str) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    let agents = load_effective_agents(&paths.agents_file())?;
    let profile = agents.profiles.get(agent).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown agent profile '{agent}'. Run `latch-cli agents` to see available profiles."
        )
    })?;

    if !command_in_path(&profile.command) {
        anyhow::bail!(
            "'{}' is not installed on PATH. Install the {} CLI first, then re-run `latch-cli login {}`.",
            profile.command,
            agent,
            agent
        );
    }

    let login_args: &[&str] = match agent {
        "claude" | "codex" | "amp" | "cursor" | "kiro" => &["login"],
        "opencode" => &["auth", "login"],
        "cline" => &["auth"],
        "goose" => &["configure"],
        _ => &[],
    };

    println!(
        "Launching {} account login ({}{})...",
        agent,
        profile.command,
        if login_args.is_empty() {
            String::new()
        } else {
            format!(" {}", login_args.join(" "))
        }
    );

    let status = Command::new(&profile.command)
        .args(login_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if !status.success() {
        anyhow::bail!("login command exited with status {status}");
    }
    Ok(())
}

pub(crate) fn cmd_init(force: bool) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    std::fs::create_dir_all(&paths.config_dir)?;
    let target = paths.agents_file();
    if target.exists() && !force {
        println!(
            "{} already exists (use --force to overwrite)",
            target.display()
        );
        return Ok(());
    }

    let template = r#"# Custom Latch agent profiles (optional).
# Built-in profiles (claude, codex, gemini, opencode, aider, goose, qwen, shell)
# already work out of the box with both account login and optional API keys.
#
# Example custom profile override:
# [profiles.my-agent]
# command = "claude"
# args = ["--model", "sonnet"]
# proxy_mode = "bypass"
# pass_env = ["ANTHROPIC_API_KEY"]
"#;
    std::fs::write(&target, template)?;
    println!("created {}", target.display());
    Ok(())
}

fn cmd_daemon_stop(json_out: bool) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    let mut client = Client::connect(&paths.socket_path)?;
    let res: Value = client.call("daemon.shutdown", json!({}))?;
    drop(client);

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && paths.socket_path.exists() {
        thread::sleep(Duration::from_millis(50));
    }

    if json_out {
        println!("{}", serde_json::to_string_pretty(&res)?);
    } else {
        println!("daemon stopped");
    }
    Ok(())
}

pub(crate) fn cmd_daemon_status(json_out: bool) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    let mut client = Client::connect(&paths.socket_path)?;
    let res: Value = client.call("daemon.status", json!({}))?;
    if json_out {
        println!("{}", serde_json::to_string_pretty(&res)?);
    } else {
        println!(
            "latchd v{} | pid={} | live_sessions={} | sandbox={}",
            res["version"].as_str().unwrap_or("?"),
            res["pid"].as_u64().unwrap_or(0),
            res["live_sessions"].as_u64().unwrap_or(0),
            res["sandbox"].as_str().unwrap_or("?"),
        );
    }
    Ok(())
}

fn cmd_project(action: ProjectCmd, json_out: bool) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    ensure_daemon_running(&paths)?;
    let mut client = Client::connect(&paths.socket_path)?;

    match action {
        ProjectCmd::Add {
            name,
            path,
            default_branch,
        } => {
            let abs = path.canonicalize().unwrap_or(path);
            let proj: Project = client.call(
                "project.add",
                json!({
                    "name": name,
                    "repo_path": abs.to_string_lossy(),
                    "default_branch": default_branch,
                }),
            )?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&proj)?);
            } else {
                println!(
                    "added project {} ({}) -> {}",
                    proj.name, proj.default_branch, proj.repo_path
                );
            }
        }
        ProjectCmd::List => {
            let list: Vec<Project> = client.call("project.list", json!({}))?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else if list.is_empty() {
                println!("no projects registered");
            } else {
                println!("{:<18} {:<12} REPO PATH", "NAME", "BRANCH");
                for p in list {
                    println!("{:<18} {:<12} {}", p.name, p.default_branch, p.repo_path);
                }
            }
        }
        ProjectCmd::Remove { name } => {
            let res: Value = client.call("project.remove", json!({ "name": name }))?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                println!("removed project {name}");
            }
        }
    }
    Ok(())
}

fn cmd_run(args: RunArgs, json_out: bool) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    ensure_daemon_running(&paths)?;
    let mut client = Client::connect(&paths.socket_path)?;

    let project_name = match args.project {
        Some(name) => name,
        None => resolve_or_register_cwd_project(&mut client)?,
    };

    let sess: Session = client.call(
        "session.create",
        json!({
            "project": project_name,
            "profile": args.profile,
            "task": args.task,
            "base_ref": args.base,
        }),
    )?;

    if json_out {
        println!("{}", serde_json::to_string_pretty(&sess)?);
    } else {
        println!(
            "started session {} (profile={}, branch={}, status={})",
            sess.id.short(),
            sess.agent_profile,
            sess.branch_name,
            sess.status
        );
    }

    if args.attach {
        attach_interactive(&paths.socket_path, sess.id.as_str())?;
    }
    Ok(())
}

pub(crate) fn resolve_or_register_cwd_project(client: &mut Client) -> anyhow::Result<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("current directory is not a git repository; pass --project <name>");
    }
    let toplevel = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let canon_top = toplevel.canonicalize().unwrap_or(toplevel);
    let top_str = canon_top.to_string_lossy().into_owned();

    let projects: Vec<Project> = client.call("project.list", json!({}))?;
    if let Some(found) = projects.into_iter().find(|p| p.repo_path == top_str) {
        return Ok(found.name);
    }

    let name = canon_top
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();
    let added: Project = client.call(
        "project.add",
        json!({
            "name": name,
            "repo_path": top_str,
        }),
    )?;
    Ok(added.name)
}

fn cmd_session(action: SessionCmd, json_out: bool) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    ensure_daemon_running(&paths)?;
    if let SessionCmd::Attach { id } = action {
        return attach_interactive(&paths.socket_path, &id);
    }

    let mut client = Client::connect(&paths.socket_path)?;
    match action {
        SessionCmd::List { project, status } => {
            let parsed_status = match status {
                Some(s) => Some(s.parse::<SessionStatus>()?),
                None => None,
            };
            let list: Vec<Session> = client.call(
                "session.list",
                json!({ "project": project, "status": parsed_status }),
            )?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else if list.is_empty() {
                println!("no sessions");
            } else {
                println!(
                    "{:<10} {:<16} {:<12} {:<24}",
                    "ID", "PROFILE", "STATUS", "BRANCH"
                );
                for s in list {
                    println!(
                        "{:<10} {:<16} {:<12} {:<24}",
                        s.id.short(),
                        s.agent_profile,
                        s.status,
                        s.branch_name
                    );
                }
            }
        }
        SessionCmd::Show { id } => {
            let s: Session = client.call("session.get", json!({ "id": id }))?;
            println!("{}", serde_json::to_string_pretty(&s)?);
        }
        SessionCmd::Pause { id } => {
            let s: Session = client.call("session.pause", json!({ "id": id }))?;
            print_session_result(&s, json_out)?;
        }
        SessionCmd::Resume { id } => {
            let s: Session = client.call("session.resume", json!({ "id": id }))?;
            print_session_result(&s, json_out)?;
        }
        SessionCmd::Kill { id } => {
            let s: Session = client.call("session.kill", json!({ "id": id }))?;
            print_session_result(&s, json_out)?;
        }
        SessionCmd::Screen { id } => {
            let res: Value = client.call("session.screen", json!({ "id": id }))?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                println!("{}", res["screen"].as_str().unwrap_or(""));
            }
        }
        SessionCmd::Diff { id, file } => {
            let res: Value = client.call("session.diff", json!({ "id": id, "file": file }))?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                if let Some(files) = res["files"].as_array() {
                    for f in files {
                        println!(
                            "{} {}",
                            f["kind"].as_str().unwrap_or("?"),
                            f["path"].as_str().unwrap_or("")
                        );
                    }
                }
                if let Some(patch) = res["patch"].as_str()
                    && !patch.is_empty()
                {
                    println!("\n{patch}");
                }
            }
        }
        SessionCmd::MergeCheck { id } => {
            let res: Value = client.call("session.merge_check", json!({ "id": id }))?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        SessionCmd::Merge {
            id,
            strategy,
            message,
        } => {
            let res: Value = client.call(
                "session.merge",
                json!({ "id": id, "strategy": strategy.as_str(), "message": message }),
            )?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                println!("merged commit {}", res["commit"].as_str().unwrap_or(""));
            }
        }
        SessionCmd::Discard { id } => {
            let res: Value = client.call("session.discard", json!({ "id": id }))?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                println!("discarded session {id}");
            }
        }
        SessionCmd::Attach { .. } => unreachable!(),
    }
    Ok(())
}

fn print_session_result(s: &Session, json_out: bool) -> anyhow::Result<()> {
    if json_out {
        println!("{}", serde_json::to_string_pretty(s)?);
    } else {
        println!("{} -> {}", s.id.short(), s.status);
    }
    Ok(())
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

pub(crate) fn attach_interactive(socket_path: &Path, id: &str) -> anyhow::Result<()> {
    let client = Client::connect(socket_path)?;
    let stream = client.attach(id)?;

    enable_raw_mode()?;
    let _guard = RawModeGuard;

    let detached = Arc::new(AtomicBool::new(false));
    let detached_in = Arc::clone(&detached);
    let sock_in = socket_path.to_path_buf();
    let sid_in = id.to_string();

    thread::Builder::new()
        .name("latch-attach-stdin".to_string())
        .spawn(move || {
            let Ok(mut in_client) = Client::connect(&sock_in) else {
                return;
            };
            let mut stdin = io::stdin();
            let mut buf = [0u8; 256];
            while !detached_in.load(Ordering::Acquire) {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Some(pos) = buf[..n].iter().position(|&b| b == 0x1d) {
                            if pos > 0 {
                                let encoded = BASE64.encode(&buf[..pos]);
                                let _ = in_client.call::<Value>(
                                    "session.input",
                                    json!({ "id": sid_in, "data_base64": encoded }),
                                );
                            }
                            detached_in.store(true, Ordering::Release);
                            break;
                        }
                        let encoded = BASE64.encode(&buf[..n]);
                        if in_client
                            .call::<Value>(
                                "session.input",
                                json!({ "id": sid_in, "data_base64": encoded }),
                            )
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })?;

    let mut stdout = io::stdout();
    for event in stream {
        if detached.load(Ordering::Acquire) {
            break;
        }
        match event? {
            AttachEvent::Snapshot(bytes) | AttachEvent::Output(bytes) => {
                stdout.write_all(&bytes)?;
                stdout.flush()?;
            }
            AttachEvent::Dropped(n) => {
                let msg = format!("\r\n[latch: dropped {n} frames]\r\n");
                stdout.write_all(msg.as_bytes())?;
                stdout.flush()?;
            }
        }
    }
    Ok(())
}

fn cmd_config_check(json_out: bool) -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    let cfg_res = load_global_config(&paths.config_file());
    let agents_res = load_agents(&paths.agents_file());

    match (cfg_res, agents_res) {
        (Ok(cfg), Ok(agents)) => {
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "valid": true,
                        "profiles": agents.profiles.keys().collect::<Vec<_>>(),
                        "tokio_workers": cfg.tokio_workers,
                    }))?
                );
            } else {
                println!(
                    "config OK ({} agent profile(s) loaded)",
                    agents.profiles.len()
                );
            }
            Ok(())
        }
        (cfg_r, agents_r) => {
            if let Err(e) = cfg_r {
                eprintln!("config error: {e}");
            }
            if let Err(e) = agents_r {
                eprintln!("agents error: {e}");
            }
            std::process::exit(1);
        }
    }
}
