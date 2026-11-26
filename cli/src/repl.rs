#![allow(clippy::print_stdout)]

use crate::agent::{self, ExecMode, NativeAgentState};
use crate::ui::{self, PromptResult, WelcomeInfo};
use latch_core::client::Client;
use latch_core::config::{Paths, load_effective_agents};
use latch_core::model::{Project, Session};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Menjalankan antarmuka CLI interaktif monokrom bergaya Antigravity CLI,
/// menggabungkan Bridge Streaming (OAuth CLI eksternal) dan Native Rust Agent (`latch`).
pub fn run() -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    crate::ensure_daemon_running(&paths)?;

    let agents = load_effective_agents(&paths.agents_file())?;
    let agent_names: Vec<String> = agents.profiles.keys().cloned().collect();
    let installed_agents: Vec<String> = agents
        .profiles
        .iter()
        .filter(|(name, p)| name.as_str() == "latch" || crate::command_in_path(&p.command))
        .map(|(k, _)| k.clone())
        .collect();

    let preferred = [
        "antigravity",
        "claude",
        "codex",
        "gemini",
        "opencode",
        "cursor",
        "amp",
        "cline",
        "copilot",
        "aider",
        "goose",
        "kiro",
        "droid",
        "qwen",
        "latch",
        "shell",
    ];
    // Prioritaskan native `latch` jika sudah login OAuth/API lokal; jika belum, pilih CLI agen terpasang.
    let mut active_agent = if agent::detect_provider(&paths).is_some() {
        "latch".to_string()
    } else {
        preferred
            .iter()
            .find(|&&name| {
                name == "latch"
                    || agents
                        .profiles
                        .get(name)
                        .is_some_and(|p| crate::command_in_path(&p.command))
            })
            .map_or_else(|| "latch".to_string(), |s| (*s).to_string())
    };

    let (workspace_name, branch_name) = detect_workspace();

    // Langsung buka sesi worktree terisolasi saat `latch-cli` dijalankan di dalam repo Git.
    let mut active_session: Option<Session> = open_session(&paths, "shell", None).ok();
    let mut exec_mode = ExecMode::Confirm;
    let mut native_state = NativeAgentState::default();

    draw_banner(
        &active_agent,
        &workspace_name,
        &branch_name,
        active_session.as_ref(),
        &installed_agents,
    );

    if let Some(ref sess) = active_session {
        ui::print_ok(&format!(
            "Session {} opened on branch {} (mode: {})",
            sess.id.short(),
            sess.branch_name,
            exec_mode.as_str()
        ));
        println!();
    }

    let mut history: Vec<String> = Vec::new();
    loop {
        let sess_short = active_session.as_ref().map(|s| s.id.short());
        let tok_suffix = if native_state.prompt_tokens + native_state.completion_tokens > 0 {
            format!(
                " · {} tok",
                native_state.prompt_tokens + native_state.completion_tokens
            )
        } else {
            String::new()
        };
        let label = format!("{} [{}]{}", active_agent, exec_mode.as_str(), tok_suffix);
        match ui::read_prompt(
            &label,
            &workspace_name,
            sess_short.as_deref(),
            &history,
            &agent_names,
        )? {
            PromptResult::Eof => {
                ui::print_info("Goodbye.");
                break;
            }
            PromptResult::CycleMode => {
                exec_mode = exec_mode.next();
                ui::print_ok(&format!("Mode -> {}", exec_mode.as_str()));
            }
            PromptResult::Clear => {
                ui::clear_screen();
                draw_banner(
                    &active_agent,
                    &workspace_name,
                    &branch_name,
                    active_session.as_ref(),
                    &installed_agents,
                );
            }
            PromptResult::Line(raw) => {
                let line = raw.trim();
                if line.is_empty() {
                    continue;
                }
                if history.last().map(String::as_str) != Some(line) {
                    history.push(line.to_string());
                }
                if let Err(err) = handle_input(
                    line,
                    &paths,
                    &mut active_agent,
                    &mut exec_mode,
                    &mut native_state,
                    &mut active_session,
                    &workspace_name,
                    &branch_name,
                    &installed_agents,
                ) {
                    ui::print_err(&err.to_string());
                }
                if matches!(line, "/exit" | "/quit") {
                    break;
                }
                println!();
            }
        }
    }

    Ok(())
}

fn open_session(paths: &Paths, profile: &str, task: Option<&str>) -> anyhow::Result<Session> {
    let mut client = Client::connect(&paths.socket_path)?;
    let project_name = crate::resolve_or_register_cwd_project(&mut client)?;
    let sess: Session = client.call(
        "session.create",
        json!({
            "project": project_name,
            "profile": profile,
            "task": task,
        }),
    )?;
    Ok(sess)
}

fn draw_banner(
    active_agent: &str,
    workspace: &str,
    branch: &str,
    active_session: Option<&Session>,
    installed_agents: &[String],
) {
    let short_owned = active_session.map(|s| s.id.short());
    let branch_owned = active_session.map(|s| s.branch_name.as_str());
    ui::print_welcome(&WelcomeInfo {
        version: env!("CARGO_PKG_VERSION"),
        workspace,
        branch,
        active_agent,
        session_short: short_owned.as_deref(),
        session_branch: branch_owned,
        installed_agents,
    });
}

fn detect_workspace() -> (String, String) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_string();

    let branch = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "no-git".to_string(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
        );
    (name, branch)
}

fn active_workdir(active_session: Option<&Session>) -> PathBuf {
    if let Some(sess) = active_session {
        let p = PathBuf::from(&sess.worktree_path);
        if p.exists() {
            return p;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Menjalankan prompt langsung via Bridge Streaming untuk CLI agen eksternal (OAuth)
/// di dalam worktree sesi aktif.
fn run_bridge_prompt(
    paths: &Paths,
    agent_name: &str,
    workdir: &Path,
    mode: ExecMode,
    prompt: &str,
) -> anyhow::Result<()> {
    let agents = load_effective_agents(&paths.agents_file())?;
    let profile = agents
        .profiles
        .get(agent_name)
        .ok_or_else(|| anyhow::anyhow!("unknown agent profile: {agent_name}"))?;

    if !crate::command_in_path(&profile.command) {
        anyhow::bail!(
            "'{}' is not installed on PATH. Switch agents with `/agent` (or select built-in `latch`).",
            profile.command
        );
    }

    let mut args: Vec<String> = profile.args.clone();
    match agent_name {
        "antigravity" => {
            args.push("-p".to_string());
            args.push(prompt.to_string());
            let agy_mode = match mode {
                ExecMode::Auto => Some("auto"),
                ExecMode::AcceptEdits => Some("accept-edits"),
                ExecMode::Plan => Some("plan"),
                ExecMode::Confirm => None,
            };
            if let Some(m) = agy_mode {
                args.push("--mode".to_string());
                args.push(m.to_string());
            }
        }
        "codex" => {
            args.push("exec".to_string());
            args.push(prompt.to_string());
        }
        "claude" | "gemini" | "copilot" | "qwen" => {
            args.push("-p".to_string());
            args.push(prompt.to_string());
        }
        "opencode" => {
            args.push("run".to_string());
            args.push(prompt.to_string());
        }
        "aider" => {
            args.push("--message".to_string());
            args.push(prompt.to_string());
            if mode == ExecMode::Auto {
                args.push("--yes".to_string());
            }
        }
        "goose" => {
            args = vec!["run".to_string(), "-t".to_string(), prompt.to_string()];
        }
        "shell" => {
            args.push("-c".to_string());
            args.push(prompt.to_string());
        }
        _ => {
            args.push(prompt.to_string());
        }
    }

    ui::print_info(&format!(
        "bridge -> {} in {}",
        agent_name,
        workdir.display()
    ));
    println!();

    let status = Command::new(&profile.command)
        .args(&args)
        .current_dir(workdir)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if !status.success() {
        ui::print_warn(&format!("{agent_name} exited with {status}"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_input(
    line: &str,
    paths: &Paths,
    active_agent: &mut String,
    exec_mode: &mut ExecMode,
    native_state: &mut NativeAgentState,
    active_session: &mut Option<Session>,
    workspace: &str,
    branch: &str,
    installed_agents: &[String],
) -> anyhow::Result<()> {
    if !line.starts_with('/') {
        let workdir = active_workdir(active_session.as_ref());
        if active_agent == "latch" {
            return agent::run_native_turn(paths, native_state, &workdir, *exec_mode, line);
        }
        return run_bridge_prompt(paths, active_agent, &workdir, *exec_mode, line);
    }

    let mut parts = line.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();

    let resolve_target = |arg: &str, current: Option<&Session>| -> anyhow::Result<String> {
        if !arg.is_empty() {
            return Ok(arg.to_string());
        }
        current
            .map(|s| s.id.to_string())
            .ok_or_else(|| anyhow::anyhow!("no active session; pass <session-id> or run `/new`"))
    };

    match cmd {
        "/exit" | "/quit" => {
            ui::print_info("Goodbye.");
            Ok(())
        }
        "/help" => {
            ui::print_help();
            Ok(())
        }
        "/clear" => {
            native_state.reset();
            ui::clear_screen();
            draw_banner(
                active_agent,
                workspace,
                branch,
                active_session.as_ref(),
                installed_agents,
            );
            Ok(())
        }
        "/mode" => {
            if rest.is_empty() {
                let items = vec![
                    (
                        "confirm".to_string(),
                        "Ask before every file write and shell command (safe default)".to_string(),
                    ),
                    (
                        "accept-edits".to_string(),
                        "Auto-approve file edits; ask before shell commands".to_string(),
                    ),
                    (
                        "auto".to_string(),
                        "Auto-approve all file edits and shell commands".to_string(),
                    ),
                    (
                        "plan".to_string(),
                        "Read-only planning mode (blocks writes and shell commands)".to_string(),
                    ),
                ];
                if let Some(idx) = ui::select_menu("Select Execution Mode", &items)?
                    && let Some(m) = ExecMode::parse(&items[idx].0)
                {
                    *exec_mode = m;
                    ui::print_ok(&format!("Execution mode set to '{}'", m.as_str()));
                }
                Ok(())
            } else if let Some(m) = ExecMode::parse(rest) {
                *exec_mode = m;
                ui::print_ok(&format!("Execution mode set to '{}'", m.as_str()));
                Ok(())
            } else {
                anyhow::bail!("invalid mode '{rest}'. Choose: confirm, accept-edits, auto, plan")
            }
        }
        "/model" | "/models" => {
            if rest.is_empty() {
                let models = vec![
                    (
                        "anthropic/claude-sonnet-4".to_string(),
                        "OpenRouter — Claude Sonnet 4 (coding default)".to_string(),
                    ),
                    (
                        "anthropic/claude-opus-4".to_string(),
                        "OpenRouter — Claude Opus 4 (deep reasoning)".to_string(),
                    ),
                    (
                        "openai/gpt-4o".to_string(),
                        "OpenRouter — OpenAI GPT-4o".to_string(),
                    ),
                    (
                        "google/gemini-2.5-pro".to_string(),
                        "OpenRouter — Google Gemini 2.5 Pro".to_string(),
                    ),
                    (
                        "deepseek/deepseek-r1".to_string(),
                        "OpenRouter — DeepSeek R1".to_string(),
                    ),
                    (
                        "qwen/qwen-2.5-coder-32b-instruct".to_string(),
                        "OpenRouter — Qwen 2.5 Coder 32B".to_string(),
                    ),
                    (
                        "gpt-4o".to_string(),
                        "GitHub Models / OpenAI Direct — GPT-4o".to_string(),
                    ),
                    (
                        "claude-sonnet-4-20250514".to_string(),
                        "Anthropic Direct — Claude Sonnet 4".to_string(),
                    ),
                    (
                        "gemini-2.5-pro".to_string(),
                        "Gemini Direct — Gemini 2.5 Pro".to_string(),
                    ),
                ];
                if let Some(idx) =
                    ui::select_menu("Select Model for Native `latch` Agent", &models)?
                {
                    let chosen = models[idx].0.clone();
                    native_state.model_override = Some(chosen.clone());
                    ui::print_ok(&format!("Model set to '{chosen}'"));
                }
            } else {
                native_state.model_override = Some(rest.to_string());
                ui::print_ok(&format!("Built-in `latch` model set to '{rest}'"));
            }
            Ok(())
        }
        "/auth" => {
            crate::auth::print_auth_status(paths);
            Ok(())
        }
        "/logout" => crate::auth::logout_interactive(paths),
        "/compact" => agent::compact_history(paths, native_state),
        "/cost" | "/tokens" => {
            let total = native_state.prompt_tokens + native_state.completion_tokens;
            ui::print_info(&format!(
                "Native session usage: {} turns · {} prompt + {} completion = {} total tokens",
                native_state.turns,
                native_state.prompt_tokens,
                native_state.completion_tokens,
                total
            ));
            Ok(())
        }
        "/undo" => {
            let workdir = active_workdir(active_session.as_ref());
            let out = Command::new("git")
                .args(["-C", &workdir.to_string_lossy(), "checkout", "--", "."])
                .output()?;
            if out.status.success() {
                ui::print_ok(&format!(
                    "Reverted tracked changes in {}",
                    workdir.display()
                ));
            } else {
                anyhow::bail!(
                    "git checkout failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            Ok(())
        }
        "/new" => {
            native_state.reset();
            let profile = if rest.is_empty() { "shell" } else { rest };
            let sess = open_session(paths, profile, None)?;
            ui::print_ok(&format!(
                "Opened session {} on branch {}",
                sess.id.short(),
                sess.branch_name
            ));
            *active_session = Some(sess);
            Ok(())
        }
        "/agent" | "/agents" => {
            if rest.is_empty() {
                let agents = load_effective_agents(&paths.agents_file())?;
                let items: Vec<(String, String)> = agents
                    .profiles
                    .iter()
                    .map(|(name, prof)| {
                        let status = if name == "latch" {
                            "built-in native agent"
                        } else if crate::command_in_path(&prof.command) {
                            "installed"
                        } else {
                            "not found"
                        };
                        let active_mark = if name == active_agent.as_str() {
                            " (active)"
                        } else {
                            ""
                        };
                        (
                            name.clone(),
                            format!("cmd: {:<12} [{}]{}", prof.command, status, active_mark),
                        )
                    })
                    .collect();
                if let Some(idx) = ui::select_menu("Select Active Agent", &items)? {
                    *active_agent = items[idx].0.clone();
                    ui::print_ok(&format!("Switched active agent to '{active_agent}'"));
                }
                Ok(())
            } else {
                let agents = load_effective_agents(&paths.agents_file())?;
                if !agents.profiles.contains_key(rest) {
                    anyhow::bail!(
                        "unknown agent '{rest}'. Run `/agent` to see all available profiles."
                    );
                }
                *active_agent = rest.to_string();
                ui::print_ok(&format!("Switched active agent to '{active_agent}'"));
                Ok(())
            }
        }
        "/login" => {
            if rest.is_empty() {
                crate::cmd_login_interactive()
            } else {
                crate::cmd_login(rest)
            }
        }
        "/init" => crate::cmd_init(rest == "--force"),
        "/run" => {
            if rest.is_empty() {
                anyhow::bail!("usage: /run [agent] <task>");
            }
            let agents = load_effective_agents(&paths.agents_file())?;
            let mut split = rest.splitn(2, char::is_whitespace);
            let first = split.next().unwrap_or("");
            let second = split.next().unwrap_or("").trim();
            let (profile, task) = if agents.profiles.contains_key(first) && !second.is_empty() {
                (first, second)
            } else {
                (active_agent.as_str(), rest)
            };
            let sess = open_session(paths, profile, Some(task))?;
            ui::print_ok(&format!(
                "Started background PTY session {} ({}) on branch {}",
                sess.id.short(),
                sess.agent_profile,
                sess.branch_name
            ));
            *active_session = Some(sess);
            Ok(())
        }
        "/sessions" | "/ls" => {
            let mut client = Client::connect(&paths.socket_path)?;
            let list: Vec<Session> = client.call("session.list", json!({}))?;
            let cur_id = active_session.as_ref().map(|s| s.id.as_str());
            ui::print_sessions(&list, cur_id);
            Ok(())
        }
        "/attach" => {
            let sid = resolve_target(rest, active_session.as_ref())?;
            ui::print_info(&format!("Attaching to {sid} (press Ctrl-] to detach)..."));
            crate::attach_interactive(&paths.socket_path, &sid)
        }
        "/screen" => {
            let sid = resolve_target(rest, active_session.as_ref())?;
            let mut client = Client::connect(&paths.socket_path)?;
            let res: Value = client.call("session.screen", json!({ "id": sid }))?;
            println!("{}", res["screen"].as_str().unwrap_or(""));
            Ok(())
        }
        "/diff" => {
            let sid = resolve_target(rest, active_session.as_ref())?;
            let mut client = Client::connect(&paths.socket_path)?;
            let res: Value = client.call("session.diff", json!({ "id": sid }))?;
            ui::print_mono_diff(res["patch"].as_str().unwrap_or(""));
            Ok(())
        }
        "/merge" => {
            let sid = resolve_target(rest, active_session.as_ref())?;
            let mut client = Client::connect(&paths.socket_path)?;
            let chk: Value = client.call("session.merge_check", json!({ "id": sid }))?;
            if !chk["clean"].as_bool().unwrap_or(false) {
                ui::print_warn(&format!("Merge conflicts detected: {}", chk["conflicts"]));
                return Ok(());
            }
            let res: Value =
                client.call("session.merge", json!({ "id": sid, "strategy": "squash" }))?;
            ui::print_ok(&format!(
                "Merged cleanly (commit {})",
                res["commit"].as_str().unwrap_or("")
            ));
            Ok(())
        }
        "/discard" => {
            let sid = resolve_target(rest, active_session.as_ref())?;
            let mut client = Client::connect(&paths.socket_path)?;
            let _: Value = client.call("session.discard", json!({ "id": sid }))?;
            ui::print_ok(&format!("Discarded session {sid}"));
            if active_session
                .as_ref()
                .is_some_and(|s| s.id.as_str() == sid)
            {
                *active_session = None;
            }
            Ok(())
        }
        "/pause" => {
            let sid = resolve_target(rest, active_session.as_ref())?;
            let mut client = Client::connect(&paths.socket_path)?;
            let s: Session = client.call("session.pause", json!({ "id": sid }))?;
            ui::print_ok(&format!("{} -> {}", s.id.short(), s.status));
            Ok(())
        }
        "/resume" => {
            let sid = resolve_target(rest, active_session.as_ref())?;
            let mut client = Client::connect(&paths.socket_path)?;
            let s: Session = client.call("session.resume", json!({ "id": sid }))?;
            ui::print_ok(&format!("{} -> {}", s.id.short(), s.status));
            Ok(())
        }
        "/kill" => {
            let sid = resolve_target(rest, active_session.as_ref())?;
            let mut client = Client::connect(&paths.socket_path)?;
            let s: Session = client.call("session.kill", json!({ "id": sid }))?;
            ui::print_ok(&format!("{} -> {}", s.id.short(), s.status));
            Ok(())
        }
        "/projects" => {
            let mut client = Client::connect(&paths.socket_path)?;
            let list: Vec<Project> = client.call("project.list", json!({}))?;
            if list.is_empty() {
                ui::print_info("No projects registered yet.");
            } else {
                for p in list {
                    println!("  {} ({}) -> {}", p.name, p.default_branch, p.repo_path);
                }
            }
            Ok(())
        }
        "/status" => crate::cmd_daemon_status(false),
        other => anyhow::bail!("unknown command '{other}'. Type `/help` for available commands."),
    }
}
