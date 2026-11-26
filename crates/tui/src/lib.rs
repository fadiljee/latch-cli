//! Antarmuka terminal (TUI) Latch berbasis ratatui.

pub mod app;
pub mod ui;

use app::{App, ConfirmAction, Mode};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use latch_core::client::{AttachEvent, Client};
use latch_core::model::Session;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// Menjalankan TUI interaktif Latch yang terhubung ke socket daemon.
pub fn run(socket_path: &Path) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::default();
    let sessions_shared = Arc::new(Mutex::new(Vec::<Session>::new()));
    let parser_shared = Arc::new(Mutex::new(vt100::Parser::new(40, 120, 1000)));
    let stop_flag = Arc::new(AtomicBool::new(false));

    // Thread poller daftar sesi setiap 1 detik.
    let sock_poll = socket_path.to_path_buf();
    let sess_poll = Arc::clone(&sessions_shared);
    let stop_poll = Arc::clone(&stop_flag);
    thread::Builder::new()
        .name("tui-poller".to_string())
        .spawn(move || {
            while !stop_poll.load(Ordering::Acquire) {
                if let Ok(mut client) = Client::connect(&sock_poll)
                    && let Ok(list) = client.call::<Vec<Session>>("session.list", json!({}))
                    && let Ok(mut guard) = sess_poll.lock()
                {
                    *guard = list;
                }
                thread::sleep(Duration::from_secs(1));
            }
        })?;

    let mut attached_sid: Option<String> = None;
    let mut attach_cancel: Option<Arc<AtomicBool>> = None;
    let mut last_pty_size: (u16, u16) = (0, 0);

    while !app.should_quit {
        if let Ok(guard) = sessions_shared.lock() {
            app.sync_sessions(guard.clone());
        }

        let current_sid = app.selected_session().map(|s| s.id.to_string());
        if current_sid != attached_sid {
            if let Some(prev) = attach_cancel.take() {
                prev.store(true, Ordering::Release);
            }
            attached_sid.clone_from(&current_sid);
            if let Ok(mut p) = parser_shared.lock() {
                *p = vt100::Parser::new(40, 120, 1000);
            }
            if let Some(sid) = current_sid.clone() {
                let cancel = Arc::new(AtomicBool::new(false));
                attach_cancel = Some(Arc::clone(&cancel));
                spawn_attach_reader(
                    socket_path.to_path_buf(),
                    sid,
                    Arc::clone(&parser_shared),
                    cancel,
                );
            }
        }

        terminal.draw(|f| {
            let pty_area = ui::draw(f, &app, &parser_shared);
            let new_size = (
                pty_area.width.saturating_sub(2),
                pty_area.height.saturating_sub(2),
            );
            if new_size.0 > 0
                && new_size.1 > 0
                && new_size != last_pty_size
                && let Some(sid) = current_sid.as_deref()
            {
                last_pty_size = new_size;
                if let Ok(mut p) = parser_shared.lock() {
                    p.screen_mut().set_size(new_size.1, new_size.0);
                }
                let _ = rpc_call(
                    socket_path,
                    "session.resize",
                    json!({ "id": sid, "cols": new_size.0, "rows": new_size.1 }),
                );
            }
        })?;

        if event::poll(Duration::from_millis(50))?
            && let TermEvent::Key(key) = event::read()?
        {
            handle_key(&mut app, socket_path, &parser_shared, key);
        }
    }

    stop_flag.store(true, Ordering::Release);
    if let Some(c) = attach_cancel.take() {
        c.store(true, Ordering::Release);
    }
    Ok(())
}

fn spawn_attach_reader(
    socket_path: PathBuf,
    sid: String,
    parser: Arc<Mutex<vt100::Parser>>,
    cancel: Arc<AtomicBool>,
) {
    let _ = thread::Builder::new()
        .name("tui-attach".to_string())
        .spawn(move || {
            let Ok(client) = Client::connect(&socket_path) else {
                return;
            };
            let Ok(stream) = client.attach(&sid) else {
                return;
            };
            for ev in stream {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                match ev {
                    Ok(AttachEvent::Snapshot(bytes) | AttachEvent::Output(bytes)) => {
                        if let Ok(mut guard) = parser.lock() {
                            guard.process(&bytes);
                        }
                    }
                    Ok(AttachEvent::Dropped(_)) => {}
                    Err(_) => break,
                }
            }
        });
}

fn handle_key(
    app: &mut App,
    socket_path: &Path,
    parser: &Arc<Mutex<vt100::Parser>>,
    key: KeyEvent,
) {
    match &app.mode {
        Mode::Attach => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(']') {
                app.mode = Mode::Normal;
                app.status_msg = Some("detached from PTY input".to_string());
                return;
            }
            if let Some(sess) = app.selected_session() {
                let bytes = key_to_bytes(key);
                if !bytes.is_empty() {
                    let encoded = BASE64.encode(&bytes);
                    let _ = rpc_call(
                        socket_path,
                        "session.input",
                        json!({ "id": sess.id.as_str(), "data_base64": encoded }),
                    );
                }
            }
        }
        Mode::Diff { .. } => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => app.scroll_diff(1),
            KeyCode::Char('k') | KeyCode::Up => app.scroll_diff(-1),
            _ => {}
        },
        Mode::Confirm(action) => {
            let action = *action;
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    execute_confirmed(app, socket_path, action);
                    app.mode = Mode::Normal;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    app.mode = Mode::Normal;
                    app.status_msg = Some("cancelled".to_string());
                }
                _ => {}
            }
        }
        Mode::Normal => match key.code {
            KeyCode::Char('q') => app.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => app.select_next(),
            KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
            KeyCode::Enter => {
                if app.selected_session().is_some() {
                    app.mode = Mode::Attach;
                    app.status_msg = Some("attached (Ctrl-] to detach)".to_string());
                }
            }
            KeyCode::Char('p') => {
                if let Some(sess) = app.selected_session() {
                    let method = if sess.status == latch_core::model::SessionStatus::Paused {
                        "session.resume"
                    } else {
                        "session.pause"
                    };
                    match rpc_call(socket_path, method, json!({ "id": sess.id.as_str() })) {
                        Ok(_) => app.status_msg = Some(format!("{method} ok")),
                        Err(e) => app.status_msg = Some(e),
                    }
                }
            }
            KeyCode::Char('x') => {
                if app.selected_session().is_some() {
                    app.mode = Mode::Confirm(ConfirmAction::Kill);
                }
            }
            KeyCode::Char('m') => {
                if app.selected_session().is_some() {
                    app.mode = Mode::Confirm(ConfirmAction::Merge);
                }
            }
            KeyCode::Char('D') => {
                if app.selected_session().is_some() {
                    app.mode = Mode::Confirm(ConfirmAction::Discard);
                }
            }
            KeyCode::Char('d') => {
                if let Some(sess) = app.selected_session() {
                    match rpc_call(
                        socket_path,
                        "session.diff",
                        json!({ "id": sess.id.as_str() }),
                    ) {
                        Ok(val) => {
                            let patch = val["patch"].as_str().unwrap_or("(no diff)").to_string();
                            app.mode = Mode::Diff {
                                content: patch,
                                scroll: 0,
                            };
                        }
                        Err(e) => app.status_msg = Some(e),
                    }
                }
            }
            KeyCode::PageUp => {
                if let Ok(mut p) = parser.lock() {
                    let cur = p.screen().scrollback();
                    p.screen_mut().set_scrollback(cur.saturating_add(10));
                }
            }
            KeyCode::PageDown => {
                if let Ok(mut p) = parser.lock() {
                    let cur = p.screen().scrollback();
                    p.screen_mut().set_scrollback(cur.saturating_sub(10));
                }
            }
            _ => {}
        },
    }
}

fn execute_confirmed(app: &mut App, socket_path: &Path, action: ConfirmAction) {
    let Some(sess) = app.selected_session() else {
        return;
    };
    let sid = sess.id.as_str().to_string();
    let res = match action {
        ConfirmAction::Kill => rpc_call(socket_path, "session.kill", json!({ "id": sid })),
        ConfirmAction::Merge => rpc_call(
            socket_path,
            "session.merge",
            json!({ "id": sid, "strategy": "squash" }),
        ),
        ConfirmAction::Discard => rpc_call(socket_path, "session.discard", json!({ "id": sid })),
    };
    match res {
        Ok(_) => app.status_msg = Some(format!("{action:?} succeeded")),
        Err(e) => app.status_msg = Some(e),
    }
}

fn rpc_call(socket_path: &Path, method: &str, params: Value) -> Result<Value, String> {
    let mut client = Client::connect(socket_path).map_err(|e| e.to_string())?;
    client.call(method, params).map_err(|e| e.to_string())
}

fn key_to_bytes(key: KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_alphabetic() {
                let b = (c.to_ascii_lowercase() as u8) - b'a' + 1;
                vec![b]
            } else {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        _ => Vec::new(),
    }
}
