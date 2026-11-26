#![allow(clippy::print_stdout)]

use crate::ui::{self, PromptResult, WelcomeInfo};
use latch_core::client::Client;
use latch_core::config::{Paths, load_effective_agents};
use latch_core::model::{Project, Session};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

/// Menjalankan antarmuka CLI interaktif bergaya Antigravity CLI dengan ikon Latch.
pub fn run() -> anyhow::Result<()> {
    let paths = Paths::resolve()?;
    crate::ensure_daemon_running(&paths)?;

    let agents = load_effective_agents(&paths.agents_file())?;
    let agent_names: Vec<String> = agents.profiles.keys().cloned().collect();

    // Pilih agen default pertama yang sudah terpasang di PATH.
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
    draw_banner(&paths, &active_agent, &workspace_name, &branch_name);

    let mut history: Vec<String> = Vec::new();
    loop {
        match ui::read_prompt(&active_agent, &workspace_name, &history, &agent_names)? {
            PromptResult::Eof => {
                ui::print_info("Goodbye.");
                break;
            }
            PromptResult::Clear => {
                ui::clear_screen();
                draw_banner(&paths, &active_agent, &workspace_name, &branch_name);
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
                    &workspace_name,
                    &branch_name,
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

fn draw_banner(paths: &Paths, active_agent: &str, workspace: &str, branch: &str) {
    let agents = load_effective_agents(&paths.agents_file()).unwrap_or_default();
    let installed_count = agents
        .profiles
        .values()
        .filter(|p| crate::command_in_path(&p.command))
        .count();
    let agent_installed = agents
        .profiles
        .get(active_agent)
        .is_some_and(|p| crate::command_in_path(&p.command));

    let (daemon_pid, live_sessions) = Client::connect(&paths.socket_path)
        .and_then(|mut c| c.call::<Value>("daemon.status", json!({})))
        .map_or((0, 0), |v| {
            (
                v["pid"].as_u64().unwrap_or(0),
                v["live_sessions"].as_u64().unwrap_or(0),
            )
        });

    ui::print_welcome(&WelcomeInfo {
        version: env!("CARGO_PKG_VERSION"),
        workspace,
        branch,
        active_agent,
        agent_installed,
        installed_count,
        daemon_pid,
        live_sessions,
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

fn handle_input(
    line: &str,
    paths: &Paths,
    active_agent: &mut String,
    workspace: &str,
    branch: &str,
) -> anyhow::Result<()> {
    if !line.starts_with('/') {
        return launch_and_attach(paths, active_agent, line);
    }

    let mut parts = line.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();

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
            draw_banner(paths, active_agent, workspace, branch);
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
            if agents.profiles.contains_key(first) && !second.is_empty() {
                launch_and_attach(paths, first, second)
            } else {
                launch_and_attach(paths, active_agent, rest)
            }
        }
        "/sessions" | "/ls" => {
            let mut client = Client::connect(&paths.socket_path)?;
            let list: Vec<Session> = client.call("session.list", json!({}))?;
            ui::print_sessions(&list);
            Ok(())
        }
        "/attach" => {
            if rest.is_empty() {
                anyhow::bail!("usage: /attach <session-id>");
            }
            ui::print_info(&format!("Attaching to {rest} (press Ctrl-] to detach)..."));
            crate::attach_interactive(&paths.socket_path, rest)
        }
        "/screen" => {
            if rest.is_empty() {
                anyhow::bail!("usage: /screen <session-id>");
            }
            let mut client = Client::connect(&paths.socket_path)?;
            let res: Value = client.call("session.screen", json!({ "id": rest }))?;
            println!("{}", res["screen"].as_str().unwrap_or(""));
            Ok(())
        }
        "/diff" => {
            if rest.is_empty() {
                anyhow::bail!("usage: /diff <session-id>");
            }
            let mut client = Client::connect(&paths.socket_path)?;
            let res: Value = client.call("session.diff", json!({ "id": rest }))?;
            ui::print_colored_diff(res["patch"].as_str().unwrap_or(""));
            Ok(())
        }
        "/merge" => {
            if rest.is_empty() {
                anyhow::bail!("usage: /merge <session-id>");
            }
            let mut client = Client::connect(&paths.socket_path)?;
            let chk: Value = client.call("session.merge_check", json!({ "id": rest }))?;
            if !chk["clean"].as_bool().unwrap_or(false) {
                ui::print_warn(&format!("Merge conflicts detected: {}", chk["conflicts"]));
                return Ok(());
            }
            let res: Value =
                client.call("session.merge", json!({ "id": rest, "strategy": "squash" }))?;
            ui::print_ok(&format!(
                "Merged cleanly (commit {})",
                res["commit"].as_str().unwrap_or("")
            ));
            Ok(())
        }
        "/discard" => {
            if rest.is_empty() {
                anyhow::bail!("usage: /discard <session-id>");
            }
            let mut client = Client::connect(&paths.socket_path)?;
            let _: Value = client.call("session.discard", json!({ "id": rest }))?;
            ui::print_ok(&format!("Discarded session {rest}"));
            Ok(())
        }
        "/pause" => {
            if rest.is_empty() {
                anyhow::bail!("usage: /pause <session-id>");
            }
            let mut client = Client::connect(&paths.socket_path)?;
            let s: Session = client.call("session.pause", json!({ "id": rest }))?;
            ui::print_ok(&format!("{} -> {}", s.id.short(), s.status));
            Ok(())
        }
        "/resume" => {
            if rest.is_empty() {
                anyhow::bail!("usage: /resume <session-id>");
            }
            let mut client = Client::connect(&paths.socket_path)?;
            let s: Session = client.call("session.resume", json!({ "id": rest }))?;
            ui::print_ok(&format!("{} -> {}", s.id.short(), s.status));
            Ok(())
        }
        "/kill" => {
            if rest.is_empty() {
                anyhow::bail!("usage: /kill <session-id>");
            }
            let mut client = Client::connect(&paths.socket_path)?;
            let s: Session = client.call("session.kill", json!({ "id": rest }))?;
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

fn launch_and_attach(paths: &Paths, profile: &str, task: &str) -> anyhow::Result<()> {
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
    ui::print_ok(&format!(
        "Started {} ({}) on branch {}",
        sess.id.short(),
        sess.agent_profile,
        sess.branch_name
    ));
    ui::print_info("Attaching to PTY (press Ctrl-] to return to Latch prompt)...");
    crate::attach_interactive(&paths.socket_path, sess.id.as_str())
}
