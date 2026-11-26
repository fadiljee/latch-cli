#![allow(clippy::print_stdout)]

use crate::ui::{self, PromptResult, WelcomeInfo};
use latch_core::client::Client;
use latch_core::config::{Paths, load_effective_agents};
use latch_core::model::{Project, Session};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

/// Menjalankan antarmuka CLI interaktif monokrom bergaya Antigravity CLI dan langsung membuka sesi.
pub fn run() -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    crate::ensure_daemon_running(&paths)?;

    let agents = load_effective_agents(&paths.agents_file())?;
    let agent_names: Vec<String> = agents.profiles.keys().cloned().collect();
    let installed_agents: Vec<String> = agents
        .profiles
        .iter()
        .filter(|(_, p)| crate::command_in_path(&p.command))
        .map(|(k, _)| k.clone())
        .collect();

    let preferred = [
        "claude", "codex", "gemini", "opencode", "aider", "goose", "qwen", "shell",
    ];
    let mut active_agent = preferred
        .iter()
        .find(|&&name| {
            agents
                .profiles
                .get(name)
                .is_some_and(|p| crate::command_in_path(&p.command))
        })
        .map_or_else(|| "shell".to_string(), |s| (*s).to_string());

    let (workspace_name, branch_name) = detect_workspace();

    // Langsung buka sesi baru di worktree terisolasi saat `latch-cli` dijalankan (bila di dalam repo Git).
    let mut active_session: Option<Session> = open_session(&paths, &active_agent, None).ok();

    draw_banner(
        &active_agent,
        &workspace_name,
        &branch_name,
        active_session.as_ref(),
        &installed_agents,
    );

    if let Some(ref sess) = active_session {
        ui::print_ok(&format!(
            "Session {} opened ({}) on branch {}",
            sess.id.short(),
            sess.agent_profile,
            sess.branch_name
        ));
        println!();
    }

    let mut history: Vec<String> = Vec::new();
    loop {
        let sess_short = active_session.as_ref().map(|s| s.id.short());
        match ui::read_prompt(
            &active_agent,
            &workspace_name,
            sess_short.as_deref(),
            &history,
            &agent_names,
        )? {
            PromptResult::Eof => {
                ui::print_info("Goodbye.");
                break;
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

#[allow(clippy::too_many_arguments)]
fn handle_input(
    line: &str,
    paths: &Paths,
    active_agent: &mut String,
    active_session: &mut Option<Session>,
    workspace: &str,
    branch: &str,
    installed_agents: &[String],
) -> anyhow::Result<()> {
    if !line.starts_with('/') {
        let sess = open_session(paths, active_agent, Some(line))?;
        ui::print_ok(&format!(
            "Started {} ({}) on branch {}",
            sess.id.short(),
            sess.agent_profile,
            sess.branch_name
        ));
        ui::print_info("Attaching to PTY (press Ctrl-] to return to Latch)...");
        let sid = sess.id.to_string();
        *active_session = Some(sess);
        return crate::attach_interactive(&paths.socket_path, &sid);
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
        "/new" => {
            let profile = if rest.is_empty() {
                active_agent.as_str()
            } else {
                rest
            };
            let sess = open_session(paths, profile, None)?;
            ui::print_ok(&format!(
                "Opened session {} ({}) on branch {}",
                sess.id.short(),
                sess.agent_profile,
                sess.branch_name
            ));
            *active_session = Some(sess);
            Ok(())
        }
        "/agent" | "/agents" => {
            if rest.is_empty() {
                crate::cmd_agents(false)
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
            let target = if rest.is_empty() {
                active_agent.as_str()
            } else {
                rest
            };
            crate::cmd_login(target)
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
                "Started {} ({}) on branch {}",
                sess.id.short(),
                sess.agent_profile,
                sess.branch_name
            ));
            ui::print_info("Attaching to PTY (press Ctrl-] to return to Latch)...");
            let sid = sess.id.to_string();
            *active_session = Some(sess);
            crate::attach_interactive(&paths.socket_path, &sid)
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
