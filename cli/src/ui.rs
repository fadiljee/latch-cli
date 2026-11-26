#![allow(clippy::print_stdout)]

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use latch_core::model::{Session, SessionStatus};
use std::io::{self, Write};

/// Ikon heksagonal Latch yang diturunkan langsung dari `assets/icon.png`.
pub const LATCH_ICON: [&str; 11] = [
    "        ▄▄██▀▀▄▄        ",
    "    ▄▄█▀  ██    ▀▀▄▄    ",
    "▄▄██▀▀█   ██▄▄▄▄▄▄  ▀▀▄▄",
    "██   ▄█   ██   ▀███    █",
    "██  ███   ████▄  ███   █",
    "██  ███  ▀▀▀▀█▄  ▀██   █",
    "██  ███▄▄▄▄▄▄██  ▄██   █",
    "██▄  ▀▀████████  █▀    █",
    "▀▀██▄▄▄  ▀███▀   █  ▄▄▀▀",
    "    ▀▀██████    ▄█▀▀    ",
    "        ▀▀██▄▄▀▀        ",
];

// Palet warna ANSI truecolor terinspirasi ikon monokrom + aksen cyan/es.
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const CYAN: &str = "\x1b[38;2;94;234;212m";
const BLUE: &str = "\x1b[38;2;125;211;252m";
const GREEN: &str = "\x1b[38;2;74;222;128m";
const YELLOW: &str = "\x1b[38;2;250;204;21m";
const RED: &str = "\x1b[38;2;248;113;113m";
const WHITE: &str = "\x1b[38;2;241;245;249m";
const MUTED: &str = "\x1b[38;2;148;163;184m";

/// Gradien vertikal lembut untuk ikon Latch.
const ICON_GRADIENT: [&str; 11] = [
    "\x1b[38;2;248;250;252m",
    "\x1b[38;2;226;242;254m",
    "\x1b[38;2;196;234;252m",
    "\x1b[38;2;165;226;248m",
    "\x1b[38;2;134;219;244m",
    "\x1b[38;2;110;218;236m",
    "\x1b[38;2;94;224;224m",
    "\x1b[38;2;94;234;212m",
    "\x1b[38;2;125;211;252m",
    "\x1b[38;2;186;230;253m",
    "\x1b[38;2;241;245;249m",
];

pub struct WelcomeInfo<'a> {
    pub version: &'a str,
    pub workspace: &'a str,
    pub branch: &'a str,
    pub active_agent: &'a str,
    pub agent_installed: bool,
    pub installed_count: usize,
    pub daemon_pid: u64,
    pub live_sessions: u64,
}

/// Menampilkan layar sambutan interaktif bergaya Antigravity CLI dengan ikon Latch.
pub fn print_welcome(info: &WelcomeInfo<'_>) {
    let agent_badge = if info.agent_installed {
        format!("{GREEN}● ready{RESET}")
    } else {
        format!("{YELLOW}○ not on PATH{RESET}")
    };

    let right_lines: [String; 11] = [
        String::new(),
        format!(
            "{BOLD}{WHITE}L A T C H{RESET}  {DIM}v{}{RESET}  {CYAN}Rust Agent Runtime{RESET}",
            info.version
        ),
        format!("{MUTED}────────────────────────────────────────────────{RESET}"),
        format!(
            "{MUTED}Workspace :{RESET} {WHITE}{}{RESET} {BLUE}({}){RESET}",
            info.workspace, info.branch
        ),
        format!(
            "{MUTED}Agent     :{RESET} {CYAN}{}{RESET} {agent_badge}  {DIM}({} installed){RESET}",
            info.active_agent, info.installed_count
        ),
        format!(
            "{MUTED}Daemon    :{RESET} {GREEN}running{RESET} {DIM}(pid {}, {} live session(s)){RESET}",
            info.daemon_pid, info.live_sessions
        ),
        String::new(),
        format!(
            "{MUTED}Commands  :{RESET} {WHITE}/agent{RESET}  {WHITE}/login{RESET}  {WHITE}/run{RESET}  {WHITE}/sessions{RESET}  {WHITE}/attach{RESET}"
        ),
        format!(
            "            {WHITE}/diff{RESET}   {WHITE}/merge{RESET}  {WHITE}/discard{RESET}  {WHITE}/status{RESET}   {WHITE}/help{RESET}"
        ),
        format!(
            "{DIM}Type a task to launch an isolated agent, or / for commands (Tab completes).{RESET}"
        ),
        String::new(),
    ];

    println!();
    for (idx, icon_row) in LATCH_ICON.iter().enumerate() {
        let color = ICON_GRADIENT[idx];
        println!("  {color}{icon_row}{RESET}    {}", right_lines[idx]);
    }
    println!();
}

pub fn clear_screen() {
    print!("\x1b[2J\x1b[H");
    let _ = io::stdout().flush();
}

pub fn print_ok(msg: &str) {
    println!("  {GREEN}✓{RESET} {WHITE}{msg}{RESET}");
}

pub fn print_info(msg: &str) {
    println!("  {CYAN}•{RESET} {MUTED}{msg}{RESET}");
}

pub fn print_warn(msg: &str) {
    println!("  {YELLOW}!{RESET} {YELLOW}{msg}{RESET}");
}

pub fn print_err(msg: &str) {
    println!("  {RED}✗{RESET} {RED}{msg}{RESET}");
}

pub fn print_sessions(sessions: &[Session]) {
    if sessions.is_empty() {
        print_info("No sessions yet. Type a task or run `/run <agent> <task>`.");
        return;
    }
    println!(
        "  {DIM}{:<10} {:<14} {:<12} {:<26} TASK{RESET}",
        "ID", "AGENT", "STATUS", "BRANCH"
    );
    for s in sessions {
        let color = match s.status {
            SessionStatus::Running => GREEN,
            SessionStatus::Paused | SessionStatus::WaitingApproval => YELLOW,
            SessionStatus::Completed => CYAN,
            SessionStatus::Tripped | SessionStatus::Failed => RED,
            SessionStatus::Hibernated => BLUE,
        };
        let task_preview = s.task.as_deref().unwrap_or("—");
        println!(
            "  {WHITE}{:<10}{RESET} {:<14} {color}{:<12}{RESET} {MUTED}{:<26}{RESET} {}",
            s.id.short(),
            s.agent_profile,
            s.status,
            s.branch_name,
            task_preview
        );
    }
}

pub fn print_colored_diff(patch: &str) {
    if patch.trim().is_empty() {
        print_info("No diff changes in this worktree.");
        return;
    }
    for line in patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff --git") {
            println!("  {BOLD}{WHITE}{line}{RESET}");
        } else if line.starts_with("@@") {
            println!("  {CYAN}{line}{RESET}");
        } else if line.starts_with('+') {
            println!("  {GREEN}{line}{RESET}");
        } else if line.starts_with('-') {
            println!("  {RED}{line}{RESET}");
        } else {
            println!("  {MUTED}{line}{RESET}");
        }
    }
}

pub fn print_help() {
    println!("  {BOLD}{WHITE}Slash Commands{RESET}");
    let cmds = [
        (
            "/run [agent] <task>",
            "Launch agent in an isolated git worktree",
        ),
        (
            "/agent [name]",
            "List agents or switch the active default agent",
        ),
        (
            "/login [agent]",
            "Sign in with your subscription account (OAuth)",
        ),
        ("/sessions", "List all active and completed sessions"),
        (
            "/attach <id>",
            "Attach interactively to a running PTY (Ctrl-] to detach)",
        ),
        (
            "/screen <id>",
            "Show current terminal snapshot of a session",
        ),
        ("/diff <id>", "Show colorized git diff for a session"),
        (
            "/merge <id>",
            "Check conflicts and squash-merge into default branch",
        ),
        ("/discard <id>", "Delete session worktree and branch"),
        ("/pause <id>", "Pause session process group (SIGSTOP)"),
        ("/resume <id>", "Resume paused session (SIGCONT)"),
        ("/kill <id>", "Terminate a running session"),
        ("/projects", "List registered Git repositories"),
        ("/status", "Show daemon status and health"),
        ("/init", "Generate starter ~/.config/latch/agents.toml"),
        ("/clear", "Clear terminal and redraw Latch banner"),
        ("/exit", "Exit Latch CLI"),
    ];
    for (cmd, desc) in cmds {
        println!("    {CYAN}{cmd:<22}{RESET} {MUTED}{desc}{RESET}");
    }
    println!();
    println!(
        "  {DIM}Or just type any task directly to launch your active agent immediately.{RESET}"
    );
}

pub const SLASH_COMMANDS: &[&str] = &[
    "/run",
    "/agent",
    "/login",
    "/sessions",
    "/attach",
    "/screen",
    "/diff",
    "/merge",
    "/discard",
    "/pause",
    "/resume",
    "/kill",
    "/projects",
    "/status",
    "/init",
    "/clear",
    "/help",
    "/exit",
    "/quit",
];

pub enum PromptResult {
    Line(String),
    Clear,
    Eof,
}

struct RawGuard;

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Membaca satu baris perintah interaktif dengan history (Up/Down), Tab autocomplete, dan styling.
pub fn read_prompt(
    agent: &str,
    workspace: &str,
    history: &[String],
    agent_names: &[String],
) -> io::Result<PromptResult> {
    let mut stdout = io::stdout();
    println!("  {MUTED}╭─{RESET} {CYAN}{agent}{RESET} {DIM}· {workspace}{RESET}");
    let prefix = format!("  {MUTED}╰─{RESET}{BOLD}{CYAN}❯{RESET} ");

    enable_raw_mode()?;
    let _guard = RawGuard;

    let mut buf = String::new();
    let mut cursor: usize = 0;
    let mut hist_idx: Option<usize> = None;

    redraw_line(&mut stdout, &prefix, &buf, cursor)?;

    loop {
        if let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event::read()?
        {
            match (code, modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    if buf.is_empty() {
                        write!(stdout, "\r\n")?;
                        stdout.flush()?;
                        return Ok(PromptResult::Eof);
                    }
                    buf.clear();
                    cursor = 0;
                    hist_idx = None;
                    redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                }
                (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                    if buf.is_empty() {
                        write!(stdout, "\r\n")?;
                        stdout.flush()?;
                        return Ok(PromptResult::Eof);
                    }
                }
                (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                    write!(stdout, "\r\n")?;
                    stdout.flush()?;
                    return Ok(PromptResult::Clear);
                }
                (KeyCode::Enter, _) => {
                    write!(stdout, "\r\n")?;
                    stdout.flush()?;
                    return Ok(PromptResult::Line(buf));
                }
                (KeyCode::Backspace, _) => {
                    if cursor > 0 {
                        let prev = prev_char_boundary(&buf, cursor);
                        buf.drain(prev..cursor);
                        cursor = prev;
                        redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                    }
                }
                (KeyCode::Delete, _) => {
                    if cursor < buf.len() {
                        let next = next_char_boundary(&buf, cursor);
                        buf.drain(cursor..next);
                        redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                    }
                }
                (KeyCode::Left, _) => {
                    if cursor > 0 {
                        cursor = prev_char_boundary(&buf, cursor);
                        redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                    }
                }
                (KeyCode::Right, _) => {
                    if cursor < buf.len() {
                        cursor = next_char_boundary(&buf, cursor);
                        redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                    }
                }
                (KeyCode::Home, _) => {
                    cursor = 0;
                    redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                }
                (KeyCode::End, _) => {
                    cursor = buf.len();
                    redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                }
                (KeyCode::Up, _) => {
                    if !history.is_empty() {
                        let next_idx = match hist_idx {
                            None => history.len() - 1,
                            Some(0) => 0,
                            Some(i) => i - 1,
                        };
                        hist_idx = Some(next_idx);
                        buf.clone_from(&history[next_idx]);
                        cursor = buf.len();
                        redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                    }
                }
                (KeyCode::Down, _) => {
                    if let Some(i) = hist_idx {
                        if i + 1 < history.len() {
                            hist_idx = Some(i + 1);
                            buf.clone_from(&history[i + 1]);
                        } else {
                            hist_idx = None;
                            buf.clear();
                        }
                        cursor = buf.len();
                        redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                    }
                }
                (KeyCode::Tab, _) => {
                    if let Some(completed) = autocomplete(&buf, agent_names) {
                        buf = completed;
                        cursor = buf.len();
                        redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                    }
                }
                (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                    buf.insert(cursor, c);
                    cursor += c.len_utf8();
                    redraw_line(&mut stdout, &prefix, &buf, cursor)?;
                }
                _ => {}
            }
        }
    }
}

fn redraw_line(stdout: &mut io::Stdout, prefix: &str, buf: &str, cursor: usize) -> io::Result<()> {
    write!(stdout, "\r\x1b[2K{prefix}{buf}")?;
    let tail_chars = buf[cursor..].chars().count();
    if tail_chars > 0 {
        write!(stdout, "\x1b[{tail_chars}D")?;
    }
    stdout.flush()
}

fn prev_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.saturating_sub(1);
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = (idx + 1).min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn autocomplete(input: &str, agent_names: &[String]) -> Option<String> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() <= 1 && !trimmed.ends_with(' ') {
        let needle = parts.first().copied().unwrap_or("/");
        let matches: Vec<&&str> = SLASH_COMMANDS
            .iter()
            .filter(|c| c.starts_with(needle))
            .collect();
        if matches.len() == 1 {
            return Some(format!("{} ", matches[0]));
        }
    } else if matches!(parts.first().copied(), Some("/agent" | "/login" | "/run")) {
        let needle = if trimmed.ends_with(' ') {
            ""
        } else {
            parts.get(1).copied().unwrap_or("")
        };
        let matches: Vec<&String> = agent_names
            .iter()
            .filter(|a| a.starts_with(needle))
            .collect();
        if matches.len() == 1 {
            return Some(format!("{} {} ", parts[0], matches[0]));
        }
    }
    None
}
