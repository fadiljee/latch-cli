use latch_core::model::Session;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    Kill,
    Merge,
    Discard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Attach,
    Confirm(ConfirmAction),
    Diff { content: String, scroll: u16 },
}

/// State utama antarmuka TUI Latch.
#[derive(Debug)]
pub struct App {
    pub sessions: Vec<Session>,
    pub selected: usize,
    pub mode: Mode,
    pub status_msg: Option<String>,
    pub should_quit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            mode: Mode::Normal,
            status_msg: None,
            should_quit: false,
        }
    }
}

impl App {
    pub fn sync_sessions(&mut self, sessions: Vec<Session>) {
        self.sessions = sessions;
        if self.sessions.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len() - 1;
        }
    }

    pub fn select_next(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = (self.selected + 1) % self.sessions.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = if self.selected == 0 {
                self.sessions.len() - 1
            } else {
                self.selected - 1
            };
        }
    }

    #[must_use]
    pub fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected)
    }

    pub fn scroll_diff(&mut self, delta: i16) {
        if let Mode::Diff { scroll, .. } = &mut self.mode {
            if delta >= 0 {
                *scroll = scroll.saturating_add(delta.unsigned_abs());
            } else {
                *scroll = scroll.saturating_sub(delta.unsigned_abs());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use latch_core::model::{ProxyMode, SessionStatus};
    use latch_core::{ProjectId, SessionId};

    fn dummy_session(profile: &str) -> Session {
        Session {
            id: SessionId::new(),
            project_id: ProjectId::new(),
            agent_profile: profile.to_string(),
            worktree_path: "/tmp/wt".to_string(),
            branch_name: "latch/demo/12345678".to_string(),
            status: SessionStatus::Running,
            pid: Some(100),
            started_at_ms: 1,
            ended_at_ms: None,
            task: None,
            exit_code: None,
            trip_reason: None,
            proxy_mode: ProxyMode::Bypass,
            sandbox_provider: "noop".to_string(),
            cleaned_at_ms: None,
        }
    }

    #[test]
    fn navigation_wraps_and_clamps_on_sync() {
        let mut app = App::default();
        app.sync_sessions(vec![dummy_session("a"), dummy_session("b")]);
        assert_eq!(app.selected, 0);
        app.select_prev();
        assert_eq!(app.selected, 1);
        app.select_next();
        assert_eq!(app.selected, 0);

        app.selected = 1;
        app.sync_sessions(vec![dummy_session("only")]);
        assert_eq!(app.selected, 0);
    }
}
