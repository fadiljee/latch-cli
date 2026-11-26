#![allow(clippy::print_stdout)]

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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
const REVERSE: &str = "\x1b[7m";
const WHITE: &str = "\x1b[38;2;250;250;250m";
const SILVER: &str = "\x1b[38;2;203;213;225m";
const GRAY: &str = "\x1b[38;2;148;163;184m";
const DARK: &str = "\x1b[38;2;82;82;91m";

const MENU_PAGE_SIZE: usize = 7;

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
            "{GRAY}Active    :{RESET} {BOLD}{WHITE}{}{RESET}  {DIM}(/agent to switch · /login for OAuth){RESET}",
            info.active_agent
        ),
        session_line,
        String::new(),
        format!(
            "{GRAY}Commands  :{RESET} {SILVER}/login  /agent  /new  /sessions  /attach  /diff{RESET}"
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
    (
        "/login",
        "OAuth sign-in (OpenRouter / GitHub / CLI) or API key",
    ),
    ("/auth", "Show saved OAuth & API key authentication status"),
    ("/logout", "Remove saved OAuth or API key credentials"),
    (
        "/agent",
        "Choose or switch active agent (native `latch` or external CLI)",
    ),
    (
        "/mode",
        "Switch execution mode (confirm, accept-edits, auto, plan) or press Tab",
    ),
    (
        "/model",
        "Choose or change LLM model for native `latch` agent",
    ),
    (
        "/compact",
        "Summarize & compact conversation context window",
    ),
    ("/undo", "Revert uncommitted changes in the active worktree"),
    ("/cost", "Show token usage for the current native session"),
    ("/new", "Open a new isolated worktree session"),
    ("/run", "Run a specific agent in a background PTY worktree"),
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
    println!(
        "  {DIM}Type `/` then use ↑/↓ or PgUp/PgDn to browse commands, Enter/Tab to select.{RESET}"
    );
}

pub enum PromptResult {
    Line(String),
    CycleMode,
    Clear,
    Eof,
}

struct RawGuard;

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// Menu pemilih interaktif monokrom (mis. untuk daftar OAuth login atau pilih agen).
pub fn select_menu(title: &str, items: &[(String, String)]) -> io::Result<Option<usize>> {
    if items.is_empty() {
        return Ok(None);
    }
    let mut stdout = io::stdout();
    let mut selected: usize = 0;

    enable_raw_mode()?;
    let _guard = RawGuard;

    let mut rendered_lines: usize = 0;
    loop {
        if rendered_lines > 0 {
            write!(stdout, "\x1b[{rendered_lines}A\r\x1b[J")?;
        }

        let start = (selected / MENU_PAGE_SIZE) * MENU_PAGE_SIZE;
        let end = (start + MENU_PAGE_SIZE).min(items.len());
        let page_num = (selected / MENU_PAGE_SIZE) + 1;
        let total_pages = items.len().div_ceil(MENU_PAGE_SIZE);

        write!(
            stdout,
            "  {BOLD}{WHITE}{title}{RESET}  {DIM}(↑/↓ select · PgUp/PgDn page {page_num}/{total_pages} · Enter confirm · Esc cancel){RESET}\r\n"
        )?;
        let mut lines = 1usize;

        for (idx, (label, desc)) in items.iter().enumerate().take(end).skip(start) {
            if idx == selected {
                write!(stdout, "  {REVERSE}{BOLD} ▸ {label:<12} {desc} {RESET}\r\n")?;
            } else {
                write!(
                    stdout,
                    "    {SILVER}{label:<12}{RESET} {GRAY}{desc}{RESET}\r\n"
                )?;
            }
            lines += 1;
        }
        stdout.flush()?;
        rendered_lines = lines;

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
                (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    write!(stdout, "\x1b[{rendered_lines}A\r\x1b[J")?;
                    stdout.flush()?;
                    return Ok(None);
                }
                (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                    selected = if selected == 0 {
                        items.len() - 1
                    } else {
                        selected - 1
                    };
                }
                (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                    selected = (selected + 1) % items.len();
                }
                (KeyCode::PageUp, _) => {
                    selected = selected.saturating_sub(MENU_PAGE_SIZE);
                }
                (KeyCode::PageDown, _) => {
                    selected = (selected + MENU_PAGE_SIZE).min(items.len() - 1);
                }
                (KeyCode::Enter, _) => {
                    write!(stdout, "\x1b[{rendered_lines}A\r\x1b[J")?;
                    stdout.flush()?;
                    return Ok(Some(selected));
                }
                _ => {}
            }
        }
    }
}

/// Prompt input bergaya Antigravity CLI dengan border monokrom, Backspace penuh,
/// serta navigasi dropdown `/` menggunakan ↑/↓ dan PgUp/PgDn.
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
    let mut menu_idx: usize = 0;
    let mut menu_dismissed = false;

    redraw_prompt(
        &mut stdout,
        &prefix,
        &rule,
        agent,
        workspace,
        &sess_tag,
        &buf,
        cursor,
        menu_idx,
        menu_dismissed,
        agent_names,
    )?;

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

            let suggestions = if menu_dismissed {
                Vec::new()
            } else {
                matching_suggestions(&buf, agent_names)
            };
            let menu_open = !suggestions.is_empty();
            if menu_open && menu_idx >= suggestions.len() {
                menu_idx = suggestions.len() - 1;
            }

            match (code, modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    if buf.is_empty() {
                        clear_below_and_finish(&mut stdout)?;
                        return Ok(PromptResult::Eof);
                    }
                    buf.clear();
                    cursor = 0;
                    hist_idx = None;
                    menu_idx = 0;
                    menu_dismissed = false;
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
                (KeyCode::Esc, _) => {
                    if menu_open {
                        menu_dismissed = true;
                    }
                }
                (KeyCode::Enter, _) => {
                    // Jika dropdown `/` terbuka dan pengguna belum menulis lengkap, pilih item tersorot.
                    if menu_open {
                        let chosen = &suggestions[menu_idx].0;
                        let trimmed = buf.trim();
                        if trimmed != chosen {
                            buf = chosen.clone();
                        }
                    }
                    clear_below_and_finish(&mut stdout)?;
                    return Ok(PromptResult::Line(buf));
                }
                (KeyCode::Backspace, _)
                | (KeyCode::Char('\x7f' | '\x08'), _)
                | (KeyCode::Char('h'), KeyModifiers::CONTROL) => {
                    if cursor > 0 {
                        let prev = prev_char_boundary(&buf, cursor);
                        buf.drain(prev..cursor);
                        cursor = prev;
                        menu_idx = 0;
                        menu_dismissed = false;
                    }
                }
                (KeyCode::Delete, _) => {
                    if cursor < buf.len() {
                        let next = next_char_boundary(&buf, cursor);
                        buf.drain(cursor..next);
                        menu_idx = 0;
                        menu_dismissed = false;
                    }
                }
                (KeyCode::Left, _) => {
                    if cursor > 0 {
                        cursor = prev_char_boundary(&buf, cursor);
                    }
                }
                (KeyCode::Right, _) => {
                    if cursor < buf.len() {
                        cursor = next_char_boundary(&buf, cursor);
                    }
                }
                (KeyCode::Home, _) => {
                    cursor = 0;
                }
                (KeyCode::End, _) => {
                    cursor = buf.len();
                }
                (KeyCode::Up, _) => {
                    if menu_open {
                        menu_idx = if menu_idx == 0 {
                            suggestions.len() - 1
                        } else {
                            menu_idx - 1
                        };
                    } else if !history.is_empty() {
                        let next_idx = match hist_idx {
                            None => history.len() - 1,
                            Some(0) => 0,
                            Some(i) => i - 1,
                        };
                        hist_idx = Some(next_idx);
                        buf.clone_from(&history[next_idx]);
                        cursor = buf.len();
                    }
                }
                (KeyCode::Down, _) => {
                    if menu_open {
                        menu_idx = (menu_idx + 1) % suggestions.len();
                    } else if let Some(i) = hist_idx {
                        if i + 1 < history.len() {
                            hist_idx = Some(i + 1);
                            buf.clone_from(&history[i + 1]);
                        } else {
                            hist_idx = None;
                            buf.clear();
                        }
                        cursor = buf.len();
                    }
                }
                (KeyCode::PageUp, _) => {
                    if menu_open {
                        menu_idx = menu_idx.saturating_sub(MENU_PAGE_SIZE);
                    }
                }
                (KeyCode::PageDown, _) => {
                    if menu_open {
                        menu_idx = (menu_idx + MENU_PAGE_SIZE).min(suggestions.len() - 1);
                    }
                }
                (KeyCode::Tab, _) => {
                    if menu_open {
                        let chosen = &suggestions[menu_idx].0;
                        buf = format!("{chosen} ");
                        cursor = buf.len();
                        menu_idx = 0;
                        menu_dismissed = false;
                    } else if buf.is_empty() {
                        // Gaya OpenCode: tekan Tab saat input kosong untuk mengganti mode eksekusi.
                        clear_below_and_finish(&mut stdout)?;
                        return Ok(PromptResult::CycleMode);
                    }
                }
                (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                    buf.insert(cursor, c);
                    cursor += c.len_utf8();
                    menu_idx = 0;
                    menu_dismissed = false;
                }
                _ => {}
            }

            redraw_prompt(
                &mut stdout,
                &prefix,
                &rule,
                agent,
                workspace,
                &sess_tag,
                &buf,
                cursor,
                menu_idx,
                menu_dismissed,
                agent_names,
            )?;
        }
    }
}

fn matching_suggestions(input: &str, agent_names: &[String]) -> Vec<(String, String)> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') {
        return Vec::new();
    }
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() <= 1 && !trimmed.ends_with(' ') {
        let needle = parts.first().copied().unwrap_or("/");
        return SLASH_MENU
            .iter()
            .filter(|(c, _)| c.starts_with(needle))
            .map(|(c, d)| ((*c).to_string(), (*d).to_string()))
            .collect();
    }
    if matches!(parts.first().copied(), Some("/agent" | "/login" | "/run")) && parts.len() <= 2 {
        let cmd = parts[0];
        let needle = if trimmed.ends_with(' ') && parts.len() == 1 {
            ""
        } else if parts.len() == 2 && !trimmed.ends_with(' ') {
            parts[1]
        } else {
            return Vec::new();
        };
        return agent_names
            .iter()
            .filter(|a| a.starts_with(needle))
            .map(|a| (format!("{cmd} {a}"), format!("Select agent '{a}'")))
            .collect();
    }
    Vec::new()
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
    menu_idx: usize,
    menu_dismissed: bool,
    agent_names: &[String],
) -> io::Result<()> {
    write!(stdout, "\r\x1b[J{prefix}{WHITE}{buf}{RESET}")?;
    write!(stdout, "\r\n  {DARK}└{rule}┘{RESET}")?;
    write!(
        stdout,
        "\r\n  {DIM}{agent} · {workspace}{sess_tag} · / commands · ↑/↓ & PgUp/PgDn select · Tab complete · Ctrl+C exit{RESET}"
    )?;

    let mut extra_lines: usize = 2;
    if !menu_dismissed {
        let suggestions = matching_suggestions(buf, agent_names);
        if !suggestions.is_empty() {
            let sel = menu_idx.min(suggestions.len() - 1);
            let start = (sel / MENU_PAGE_SIZE) * MENU_PAGE_SIZE;
            let end = (start + MENU_PAGE_SIZE).min(suggestions.len());
            let page = (sel / MENU_PAGE_SIZE) + 1;
            let total_pages = suggestions.len().div_ceil(MENU_PAGE_SIZE);

            for (idx, (cmd, desc)) in suggestions.iter().enumerate().take(end).skip(start) {
                if idx == sel {
                    write!(stdout, "\r\n    {REVERSE}{BOLD} ▸ {cmd:<16} {desc} {RESET}")?;
                } else {
                    write!(
                        stdout,
                        "\r\n      {SILVER}{cmd:<16}{RESET} {DIM}{desc}{RESET}"
                    )?;
                }
                extra_lines += 1;
            }
            if total_pages > 1 {
                write!(
                    stdout,
                    "\r\n      {DIM}page {page}/{total_pages} — PgUp/PgDn for more ({} commands){RESET}",
                    suggestions.len()
                )?;
                extra_lines += 1;
            }
        }
    }

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
