use crate::app::{App, ConfirmAction, Mode};
use latch_core::model::SessionStatus;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use std::sync::{Arc, Mutex};
use tui_term::widget::PseudoTerminal;

/// Merender seluruh tata letak TUI Latch dan mengembalikan area panel PTY.
pub fn draw(f: &mut Frame<'_>, app: &App, parser: &Arc<Mutex<vt100::Parser>>) -> Rect {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(f.area());

    let main_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(20)])
        .split(outer[0]);

    draw_sessions(f, app, main_cols[0]);
    draw_pty(f, app, parser, main_cols[1]);
    draw_footer(f, app, outer[1]);

    if let Mode::Confirm(action) = app.mode {
        draw_confirm(f, action);
    } else if let Mode::Diff {
        ref content,
        scroll,
    } = app.mode
    {
        draw_diff(f, content, scroll);
    }

    main_cols[1]
}

fn draw_sessions(f: &mut Frame<'_>, app: &App, area: Rect) {
    let items: Vec<ListItem<'_>> = app
        .sessions
        .iter()
        .map(|s| {
            let color = match s.status {
                SessionStatus::Running => Color::Green,
                SessionStatus::Paused | SessionStatus::WaitingApproval => Color::Yellow,
                SessionStatus::Completed => Color::Cyan,
                SessionStatus::Tripped | SessionStatus::Failed => Color::Red,
                SessionStatus::Hibernated => Color::Blue,
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", s.id.short()),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{} ", s.agent_profile)),
                Span::styled(format!("[{}] ", s.status), Style::default().fg(color)),
                Span::styled("—", Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().title("Sessions").borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default();
    if !app.sessions.is_empty() {
        state.select(Some(app.selected));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_pty(f: &mut Frame<'_>, app: &App, parser: &Arc<Mutex<vt100::Parser>>, area: Rect) {
    let title = match app.selected_session() {
        Some(s) => format!("PTY: {} ({})", s.agent_profile, s.id.short()),
        None => "PTY (no session selected)".to_string(),
    };
    let border_style = if app.mode == Mode::Attach {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    if let Ok(guard) = parser.lock() {
        let term = PseudoTerminal::new(guard.screen()).block(block);
        f.render_widget(term, area);
    } else {
        f.render_widget(block, area);
    }
}

fn draw_footer(f: &mut Frame<'_>, app: &App, area: Rect) {
    let hints = match app.mode {
        Mode::Attach => "ATTACH MODE | Ctrl-] detach",
        Mode::Confirm(_) => "CONFIRM | y: yes | n/Esc: cancel",
        Mode::Diff { .. } => "DIFF | j/k scroll | Esc close",
        Mode::Normal => {
            "j/k nav | Enter attach | p pause | x kill | d diff | m merge | D discard | q quit"
        }
    };
    let text = match &app.status_msg {
        Some(msg) => format!("{hints}  |  {msg}"),
        None => hints.to_string(),
    };
    f.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::Gray)),
        area,
    );
}

fn draw_confirm(f: &mut Frame<'_>, action: ConfirmAction) {
    let label = match action {
        ConfirmAction::Kill => "Kill selected session? (y/n)",
        ConfirmAction::Merge => "Squash-merge selected session? (y/n)",
        ConfirmAction::Discard => "Discard selected worktree & branch? (y/n)",
    };
    let popup = centered_rect(50, 20, f.area());
    f.render_widget(Clear, popup);
    let para = Paragraph::new(label).block(Block::default().title("Confirm").borders(Borders::ALL));
    f.render_widget(para, popup);
}

fn draw_diff(f: &mut Frame<'_>, content: &str, scroll: u16) {
    let popup = centered_rect(85, 80, f.area());
    f.render_widget(Clear, popup);
    let para = Paragraph::new(content).scroll((scroll, 0)).block(
        Block::default()
            .title("Diff (Esc to close)")
            .borders(Borders::ALL),
    );
    f.render_widget(para, popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vert[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use latch_core::model::{ProxyMode, Session};
    use latch_core::{ProjectId, SessionId};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn renders_sessions_and_pty_panel_in_test_backend() {
        let mut app = App::default();
        app.sync_sessions(vec![Session {
            id: SessionId::from_string("0193a8b0-1234-7000-8000-abcdef012345"),
            project_id: ProjectId::new(),
            agent_profile: "coder-claude".to_string(),
            worktree_path: "/tmp/wt".to_string(),
            branch_name: "latch/demo/ef012345".to_string(),
            status: SessionStatus::Running,
            pid: Some(42),
            started_at_ms: 1,
            ended_at_ms: None,
            task: None,
            exit_code: None,
            trip_reason: None,
            proxy_mode: ProxyMode::Bypass,
            sandbox_provider: "noop".to_string(),
            cleaned_at_ms: None,
        }]);

        let parser = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 100)));
        parser.lock().unwrap().process(b"hello from pty");

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw(f, &app, &parser);
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        let all_text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(all_text.contains("Sessions"));
        assert!(all_text.contains("coder-claude"));
        assert!(all_text.contains("hello from pty"));
    }
}
