#![allow(clippy::print_stdout)]

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};
use latch_core::model::Session;
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

// Palet monokrom hitam-putih murni (grayscale).
const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const WHITE: &str = "\x1b[38;2;250;250;250m";
const SILVER: &str = "\x1b[38;2;203;213;225m";
const GRAY: &str = "\x1b[38;2;148;163;184m";
const DARK: &str = "\x1b[38;2;82;82;91m";

pub struct WelcomeInfo<'a> {
    pub version: &'a str,
    pub workspace: &'a str,
    pub branch: &'a str,
    pub active_agent: &'a str,
    pub session_short: Option<&'a str>,
    pub session_branch: Option<&'a str>,
    pub installed_agents: &'a [String],
}

/// Menampilkan header monokrom bergaya Antigravity CLI dengan ikon Latch.
pub fn print_welcome(info: &WelcomeInfo<'_>) {
    let session_line = match (info.session_short, info.session_branch) {
        (Some(sid), Some(br)) => {
            format!("{GRAY}Session   :{RESET} {WHITE}{sid}{RESET} {DIM}({br}){RESET}")
        }
        _ => format!("{GRAY}Session   :{RESET} {DIM}ready (not in a git repo){RESET}"),
    };

    let installed_str = if info.installed_agents.is_empty() {
        "none detected on PATH".to_string()
    } else {
        info.installed_agents.join(" · ")
    };

    let right_lines: [String; 11] = [
        String::new(),
        format!(
            "{BOLD}{WHITE}L A T C H{RESET}  {DIM}v{}{RESET}",
            info.version
        ),
        format!("{DARK}────────────────────────────────────────────────────{RESET}"),
        format!(
            "{GRAY}Workspace :{RESET} {WHITE}{}{RESET} {DIM}({}){RESET}",
            info.workspace, info.branch
        ),
        format!(
            "{GRAY}Agents    :{RESET} {WHITE}All Agents Supported{RESET} {DIM}({installed_str}){RESET}"
        ),
        format!(
            "{GRAY}Active    :{RESET} {BOLD}{WHITE}{}{RESET}  {DIM}(/agent to switch · /login to sign in){RESET}",
            info.active_agent
        ),
        session_line,
        String::new(),
        format!(
            "{GRAY}Commands  :{RESET} {SILVER}/new  /agent  /login  /sessions  /attach  /diff{RESET}"
        ),
        format!("            {SILVER}/merge  /discard  /pause  /kill  /status  /help{RESET}"),
        String::new(),
    ];

    println!();
    for (idx, icon_row) in LATCH_ICON.iter().enumerate() {
        let tone = if idx <= 1 || idx >= 9 { WHITE } else { SILVER };
        println!("  {tone}{icon_row}{RESET}    {}", right_lines[idx]);
    }
    println!();
}

pub fn clear_screen() {
    print!("\x1b[2J\x1b[H");
    let _ = io::stdout().flush();
}

pub fn print_ok(msg: &str) {
    println!("  {WHITE}✓{RESET} {SILVER}{msg}{RESET}");
}

pub fn print_info(msg: &str) {
    println!("  {GRAY}•{RESET} {GRAY}{msg}{RESET}");
}

pub fn print_warn(msg: &str) {
    println!("  {WHITE}!{RESET} {SILVER}{msg}{RESET}");
}

pub fn print_err(msg: &str) {
    println!("  {BOLD}{WHITE}✗{RESET} {WHITE}{msg}{RESET}");
}

pub fn print_sessions(sessions: &[Session], active_id: Option<&str>) {
    if sessions.is_empty() {
        print_info("No sessions yet. Run `/new` or type a task.");
        return;
    }
    println!(
        "  {DIM}{:<2} {:<10} {:<12} {:<12} {:<26} TASK{RESET}",
        "", "ID", "AGENT", "STATUS", "BRANCH"
    );
    for s in sessions {
        let marker = if active_id == Some(s.id.as_str()) {
            "▸"
        } else {
            " "
        };
        let task_preview = s.task.as_deref().unwrap_or("—");
        println!(
            "  {WHITE}{marker} {:<10}{RESET} {SILVER}{:<12}{RESET} {GRAY}{:<12}{RESET} {DIM}{:<26}{RESET} {SILVER}{}{RESET}",
            s.id.short(),
            s.agent_profile,
            s.status,
            s.branch_name,
            task_preview
        );
    }
}

pub fn print_mono_diff(patch: &str) {
    if patch.trim().is_empty() {
        print_info("No diff changes in this worktree.");
        return;
    }
    for line in patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff --git") {
            println!("  {BOLD}{WHITE}{line}{RESET}");
        } else if line.starts_with("@@") || line.starts_with('+') {
            println!("  {WHITE}{line}{RESET}");
        } else if line.starts_with('-') {
            println!("  {DIM}{line}{RESET}");
        } else {
            println!("  {GRAY}{line}{RESET}");
        }
    }
}

pub const SLASH_MENU: &[(&str, &str)] = &[
    ("/new", "Open a new isolated worktree session"),
    ("/agent", "List all agents or switch active agent"),
    ("/login", "Sign in to an agent with your account (OAuth)"),
    ("/run", "Run a specific agent on a task"),
    ("/sessions", "List all active and completed sessions"),
    (
        "/attach",
        "Attach interactively to the active/target PTY (Ctrl-])",
    ),
    ("/screen", "Show current terminal snapshot"),
    ("/diff", "Show unified git diff for the session"),
    (
        "/merge",
        "Check conflicts and squash-merge into default branch",
    ),
    ("/discard", "Delete session worktree and branch"),
    ("/pause", "Pause session process group"),
    ("/resume", "Resume paused session"),
    ("/kill", "Terminate session"),
    ("/projects", "List registered Git repositories"),
    ("/status", "Show daemon status"),
    ("/init", "Generate ~/.config/latch/agents.toml"),
    ("/clear", "Clear screen and redraw header"),
    ("/help", "Show all slash commands"),
    ("/exit", "Exit Latch CLI"),
];

pub fn print_help() {
    println!("  {BOLD}{WHITE}Commands{RESET}");
    for (cmd, desc) in SLASH_MENU {
        println!("    {WHITE}{cmd:<14}{RESET} {GRAY}{desc}{RESET}");
    }
    println!();
    println!("  {DIM}Type any prompt directly to send it to your active agent session.{RESET}");
}

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

/// Prompt input bergaya Antigravity CLI dengan border monokrom dan dropdown slash-command live.
pub fn read_prompt(
    agent: &str,
    workspace: &str,
    session_short: Option<&str>,
    history: &[String],
    agent_names: &[String],
) -> io::Result<PromptResult> {
    let mut stdout = io::stdout();
    let width = terminal::size()
        .map(|(w, _)| usize::from(w.saturating_sub(4)).clamp(48, 96))
        .unwrap_or(76);
    let rule = "─".repeat(width);
    let sess_tag = session_short.map_or_else(String::new, |s| format!(" · session {s}"));

    println!("  {DARK}┌{rule}┐{RESET}");
    let prefix = format!("  {DARK}│{RESET} {BOLD}{WHITE}❯{RESET} ");

    enable_raw_mode()?;
    let _guard = RawGuard;

    let mut buf = String::new();
    let mut cursor: usize = 0;
    let mut hist_idx: Option<usize> = None;

    redraw_prompt(
        &mut stdout,
        &prefix,
        &rule,
        agent,
        workspace,
        &sess_tag,
        &buf,
        cursor,
    )?;

    loop {
        if let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event::read()?
        {
            match (code, modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    if buf.is_empty() {
                        clear_below_and_finish(&mut stdout)?;
                        return Ok(PromptResult::Eof);
                    }
                    buf.clear();
                    cursor = 0;
                    hist_idx = None;
                    redraw_prompt(
                        &mut stdout,
                        &prefix,
                        &rule,
                        agent,
                        workspace,
                        &sess_tag,
                        &buf,
                        cursor,
                    )?;
                }
                (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                    if buf.is_empty() {
                        clear_below_and_finish(&mut stdout)?;
                        return Ok(PromptResult::Eof);
                    }
                }
                (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                    clear_below_and_finish(&mut stdout)?;
                    return Ok(PromptResult::Clear);
                }
                (KeyCode::Enter, _) => {
                    clear_below_and_finish(&mut stdout)?;
                    return Ok(PromptResult::Line(buf));
                }
                (KeyCode::Backspace, _) => {
                    if cursor > 0 {
                        let prev = prev_char_boundary(&buf, cursor);
                        buf.drain(prev..cursor);
                        cursor = prev;
                        redraw_prompt(
                            &mut stdout,
                            &prefix,
                            &rule,
                            agent,
                            workspace,
                            &sess_tag,
                            &buf,
                            cursor,
                        )?;
                    }
                }
                (KeyCode::Delete, _) => {
                    if cursor < buf.len() {
                        let next = next_char_boundary(&buf, cursor);
                        buf.drain(cursor..next);
                        redraw_prompt(
                            &mut stdout,
                            &prefix,
                            &rule,
                            agent,
                            workspace,
                            &sess_tag,
                            &buf,
                            cursor,
                        )?;
                    }
                }
                (KeyCode::Left, _) => {
                    if cursor > 0 {
                        cursor = prev_char_boundary(&buf, cursor);
                        redraw_prompt(
                            &mut stdout,
                            &prefix,
                            &rule,
                            agent,
                            workspace,
                            &sess_tag,
                            &buf,
                            cursor,
                        )?;
                    }
                }
                (KeyCode::Right, _) => {
                    if cursor < buf.len() {
                        cursor = next_char_boundary(&buf, cursor);
                        redraw_prompt(
                            &mut stdout,
                            &prefix,
                            &rule,
                            agent,
                            workspace,
                            &sess_tag,
                            &buf,
                            cursor,
                        )?;
                    }
                }
                (KeyCode::Home, _) => {
                    cursor = 0;
                    redraw_prompt(
                        &mut stdout,
                        &prefix,
                        &rule,
                        agent,
                        workspace,
                        &sess_tag,
                        &buf,
                        cursor,
                    )?;
                }
                (KeyCode::End, _) => {
                    cursor = buf.len();
                    redraw_prompt(
                        &mut stdout,
                        &prefix,
                        &rule,
                        agent,
                        workspace,
                        &sess_tag,
                        &buf,
                        cursor,
                    )?;
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
                        redraw_prompt(
                            &mut stdout,
                            &prefix,
                            &rule,
                            agent,
                            workspace,
                            &sess_tag,
                            &buf,
                            cursor,
                        )?;
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
                        redraw_prompt(
                            &mut stdout,
                            &prefix,
                            &rule,
                            agent,
                            workspace,
                            &sess_tag,
                            &buf,
                            cursor,
                        )?;
                    }
                }
                (KeyCode::Tab, _) => {
                    if let Some(completed) = autocomplete(&buf, agent_names) {
                        buf = completed;
                        cursor = buf.len();
                        redraw_prompt(
                            &mut stdout,
                            &prefix,
                            &rule,
                            agent,
                            workspace,
                            &sess_tag,
                            &buf,
                            cursor,
                        )?;
                    }
                }
                (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                    buf.insert(cursor, c);
                    cursor += c.len_utf8();
                    redraw_prompt(
                        &mut stdout,
                        &prefix,
                        &rule,
                        agent,
                        workspace,
                        &sess_tag,
                        &buf,
                        cursor,
                    )?;
                }
                _ => {}
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn redraw_prompt(
    stdout: &mut io::Stdout,
    prefix: &str,
    rule: &str,
    agent: &str,
    workspace: &str,
    sess_tag: &str,
    buf: &str,
    cursor: usize,
) -> io::Result<()> {
    // Baris input utama + border bawah + status bar + dropdown slash command jika mengetik `/`
    write!(stdout, "\r\x1b[J{prefix}{WHITE}{buf}{RESET}")?;
    write!(stdout, "\r\n  {DARK}└{rule}┘{RESET}")?;
    write!(
        stdout,
        "\r\n  {DIM}{agent} · {workspace}{sess_tag} · / commands · Tab complete · Ctrl+C exit{RESET}"
    )?;

    let mut extra_lines: usize = 2;
    let trimmed = buf.trim_start();
    if trimmed.starts_with('/') && !trimmed.contains(' ') {
        let matches: Vec<(&str, &str)> = SLASH_MENU
            .iter()
            .copied()
            .filter(|(c, _)| c.starts_with(trimmed))
            .take(6)
            .collect();
        for (cmd, desc) in matches {
            write!(stdout, "\r\n    {WHITE}{cmd:<12}{RESET} {DIM}{desc}{RESET}")?;
            extra_lines += 1;
        }
    }

    // Kembalikan kursor ke baris input pada posisi kolom yang tepat.
    write!(stdout, "\x1b[{extra_lines}A\r")?;
    let visual_col = 6 + buf[..cursor].chars().count();
    write!(stdout, "\x1b[{visual_col}C")?;
    stdout.flush()
}

fn clear_below_and_finish(stdout: &mut io::Stdout) -> io::Result<()> {
    write!(stdout, "\r\n\x1b[J")?;
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
        let matches: Vec<&str> = SLASH_MENU
            .iter()
            .map(|(c, _)| *c)
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
