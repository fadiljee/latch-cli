use latch_core::config::{GlobalConfig, Paths, load_effective_agents};
use latch_core::model::{Project, Session, SessionStatus};
use latch_core::protocol::{Event, err_code};
use latch_core::{ProjectId, SandboxPolicy, SandboxProvider, SessionId, SpawnSpec, can_transition};
use latch_git::{DiffOptions, DiffPage, GitError, MergeCheckResult, MergeStrategy};
use latch_pty::{PtyOptions, PtySession, PtySize};
use latch_store::{Store, now_epoch_ms};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::broadcast;

#[derive(Debug, Error)]
pub enum ManagerError {
    #[error("{code}: {message}")]
    Rpc {
        code: &'static str,
        message: String,
        details: Option<Value>,
    },
}

impl ManagerError {
    fn rpc(code: &'static str, message: impl Into<String>) -> Self {
        Self::Rpc {
            code,
            message: message.into(),
            details: None,
        }
    }

    fn internal(msg: impl ToString) -> Self {
        Self::rpc(err_code::INTERNAL, msg.to_string())
    }
}

/// Mengorkestrasi proyek, sesi PTY, worktree Git, dan state SQLite.
pub struct SessionManager {
    store: Store,
    paths: Paths,
    cfg: GlobalConfig,
    sandbox: Arc<dyn SandboxProvider>,
    live: Mutex<HashMap<SessionId, Arc<PtySession>>>,
    events: broadcast::Sender<Event>,
}

impl SessionManager {
    #[must_use]
    pub fn new(
        store: Store,
        paths: Paths,
        cfg: GlobalConfig,
        sandbox: Arc<dyn SandboxProvider>,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            store,
            paths,
            cfg,
            sandbox,
            live: Mutex::new(HashMap::new()),
            events,
        }
    }

    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    #[must_use]
    pub fn live_count(&self) -> usize {
        self.live.lock().map_or(0, |g| g.len())
    }

    #[must_use]
    pub fn sandbox_name(&self) -> &'static str {
        self.sandbox.name()
    }

    pub async fn project_add(
        &self,
        name: String,
        repo_path: String,
        default_branch: Option<String>,
    ) -> Result<Project, ManagerError> {
        if !latch_core::config::is_valid_identifier(&name) {
            return Err(ManagerError::rpc(
                err_code::INVALID_REQUEST,
                "project name must match [A-Za-z0-9._-]+",
            ));
        }
        let path = PathBuf::from(&repo_path);
        let toplevel = tokio::task::spawn_blocking(move || latch_git::repo_toplevel(&path))
            .await
            .map_err(ManagerError::internal)?
            .map_err(|e| ManagerError::rpc(err_code::INVALID_REQUEST, e.to_string()))?;

        let branch = match default_branch {
            Some(b) if !b.trim().is_empty() => b,
            _ => {
                let top_clone = toplevel.clone();
                tokio::task::spawn_blocking(move || latch_git::current_branch(&top_clone))
                    .await
                    .map_err(ManagerError::internal)?
                    .unwrap_or_else(|_| "main".to_string())
            }
        };

        let project = Project {
            id: ProjectId::new(),
            name,
            repo_path: toplevel.to_string_lossy().into_owned(),
            default_branch: branch,
            created_at_ms: now_epoch_ms(),
        };
        let proj_insert = project.clone();
        self.store
            .call(move |db| db.projects_insert(&proj_insert))
            .await
            .map_err(|e| ManagerError::rpc(err_code::INVALID_REQUEST, e.to_string()))?;

        Ok(project)
    }

    pub async fn project_list(&self) -> Result<Vec<Project>, ManagerError> {
        self.store
            .call(|db| db.projects_list())
            .await
            .map_err(ManagerError::internal)
    }

    pub async fn project_remove(&self, name: &str) -> Result<(), ManagerError> {
        let name_owned = name.to_string();
        let proj = self
            .store
            .call(move |db| db.projects_get_by_name(&name_owned))
            .await
            .map_err(ManagerError::internal)?
            .ok_or_else(|| {
                ManagerError::rpc(
                    err_code::PROJECT_NOT_FOUND,
                    format!("project not found: {name}"),
                )
            })?;

        let pid = proj.id.clone();
        let sessions = self
            .store
            .call(move |db| db.sessions_list(Some(&pid), None))
            .await
            .map_err(ManagerError::internal)?;

        if sessions.iter().any(|s| !s.status.is_terminal()) {
            return Err(ManagerError::rpc(
                err_code::INVALID_STATE_TRANSITION,
                "cannot remove project with active sessions",
            ));
        }

        let pid_del = proj.id;
        self.store
            .call(move |db| db.projects_delete(&pid_del))
            .await
            .map_err(ManagerError::internal)?;
        Ok(())
    }

    pub async fn session_create(
        self: &Arc<Self>,
        project_name: &str,
        profile_name: &str,
        task: Option<String>,
        base_ref: Option<String>,
    ) -> Result<Session, ManagerError> {
        let agents = load_effective_agents(&self.paths.agents_file())
            .map_err(|e| ManagerError::rpc(err_code::PROFILE_INVALID, e.to_string()))?;
        let profile = agents.profiles.get(profile_name).cloned().ok_or_else(|| {
            ManagerError::rpc(
                err_code::PROFILE_INVALID,
                format!("unknown agent profile: {profile_name}"),
            )
        })?;

        let proj_name_owned = project_name.to_string();
        let project = self
            .store
            .call(move |db| db.projects_get_by_name(&proj_name_owned))
            .await
            .map_err(ManagerError::internal)?
            .ok_or_else(|| {
                ManagerError::rpc(
                    err_code::PROJECT_NOT_FOUND,
                    format!("project not found: {project_name}"),
                )
            })?;

        let session_id = SessionId::new();
        let wt_path = self
            .paths
            .worktrees_dir()
            .join(&project.name)
            .join(session_id.as_str());
        let branch = format!("latch/{}/{}", project.name, session_id.short());
        let base = base_ref.unwrap_or_else(|| project.default_branch.clone());

        let repo_path = PathBuf::from(&project.repo_path);
        let wt_clone = wt_path.clone();
        let branch_clone = branch.clone();
        let base_clone = base.clone();
        tokio::task::spawn_blocking(move || {
            latch_git::create_worktree(&repo_path, &wt_clone, &branch_clone, &base_clone)
        })
        .await
        .map_err(ManagerError::internal)?
        .map_err(|e| ManagerError::rpc(err_code::INVALID_REQUEST, e.to_string()))?;

        let mut env = build_clean_env(&profile.pass_env, &profile.env);
        env.push(("LATCH_SESSION_ID".to_string(), session_id.to_string()));
        if let Some(ref t) = task {
            env.push(("LATCH_TASK".to_string(), t.clone()));
        }

        let mut spec = SpawnSpec {
            program: profile.command.clone(),
            args: profile.args.clone(),
            env,
            cwd: wt_path.clone(),
        };

        let policy = SandboxPolicy {
            rw_paths: vec![wt_path.clone()],
            ..SandboxPolicy::default()
        };
        if let Err(err) = self.sandbox.wrap_command(&policy, &mut spec) {
            self.cleanup_failed_worktree(&project.repo_path, &wt_path, &branch)
                .await;
            return Err(ManagerError::internal(err));
        }

        let pty_size = PtySize {
            cols: self.cfg.default_cols,
            rows: self.cfg.default_rows,
        };
        let pty_opts = PtyOptions {
            ring_bytes: self.cfg.ring_buffer_bytes,
            log_path: Some(
                self.paths
                    .logs_dir()
                    .join(format!("pty-{}.log", session_id.short())),
            ),
            log_max_bytes: self.cfg.pty_log_max_bytes,
            log_max_files: self.cfg.log_max_files,
        };

        let (pty, exit_rx) = match PtySession::spawn(spec, pty_size, pty_opts) {
            Ok(pair) => pair,
            Err(err) => {
                self.cleanup_failed_worktree(&project.repo_path, &wt_path, &branch)
                    .await;
                return Err(ManagerError::internal(err));
            }
        };

        let pid = pty.pid();
        let session = Session {
            id: session_id.clone(),
            project_id: project.id.clone(),
            agent_profile: profile_name.to_string(),
            worktree_path: wt_path.to_string_lossy().into_owned(),
            branch_name: branch,
            status: SessionStatus::Running,
            pid: Some(pid),
            started_at_ms: now_epoch_ms(),
            ended_at_ms: None,
            task,
            exit_code: None,
            trip_reason: None,
            proxy_mode: profile.proxy_mode,
            sandbox_provider: self.sandbox.name().to_string(),
            cleaned_at_ms: None,
        };

        let sess_insert = session.clone();
        if let Err(err) = self
            .store
            .call(move |db| db.sessions_insert(&sess_insert))
            .await
        {
            let pty_arc = Arc::new(pty);
            let grace = Duration::from_millis(self.cfg.kill_grace_ms);
            let _ = tokio::task::spawn_blocking(move || pty_arc.terminate(grace)).await;
            self.cleanup_failed_worktree(&project.repo_path, &wt_path, &session.branch_name)
                .await;
            return Err(ManagerError::internal(err));
        }

        {
            let mut guard = self
                .live
                .lock()
                .map_err(|_| ManagerError::internal("live session lock poisoned"))?;
            guard.insert(session_id.clone(), Arc::new(pty));
        }

        let _ = self.events.send(Event::SessionCreated {
            session: Box::new(session.clone()),
        });

        let mgr = Arc::clone(self);
        let sid_watch = session_id;
        tokio::spawn(async move {
            let exit_info = exit_rx.await.ok();
            mgr.handle_process_exit(&sid_watch, exit_info).await;
        });

        Ok(session)
    }

    async fn cleanup_failed_worktree(&self, repo_path: &str, wt_path: &Path, branch: &str) {
        let repo = PathBuf::from(repo_path);
        let wt = wt_path.to_path_buf();
        let br = branch.to_string();
        let root = self.paths.worktrees_dir();
        let _ = tokio::task::spawn_blocking(move || {
            latch_git::remove_worktree(&repo, &wt, &br, Some(&root))
        })
        .await;
    }

    async fn handle_process_exit(&self, session_id: &SessionId, exit: Option<latch_pty::ExitInfo>) {
        if let Ok(mut guard) = self.live.lock() {
            guard.remove(session_id);
        }

        let sid = session_id.clone();
        let current = match self.store.call(move |db| db.sessions_get(&sid)).await {
            Ok(Some(s)) => s,
            _ => return,
        };

        if current.status.is_terminal() {
            return;
        }

        let (to_status, exit_code) = match exit {
            Some(info) if info.success => (SessionStatus::Completed, Some(info.code)),
            Some(info) => (SessionStatus::Failed, Some(info.code)),
            None => (SessionStatus::Failed, None),
        };

        if !can_transition(current.status, to_status) {
            tracing::warn!(
                session_id = %session_id,
                from = %current.status,
                to = %to_status,
                "unexpected state transition on process exit"
            );
        }

        let sid_upd = session_id.clone();
        let now = now_epoch_ms();
        let _ = self
            .store
            .call(move |db| {
                db.sessions_update_status(&sid_upd, to_status, None, exit_code, Some(now))
            })
            .await;

        let _ = self.events.send(Event::SessionStatus {
            session_id: session_id.to_string(),
            from: current.status,
            to: to_status,
            reason: None,
        });
    }

    pub async fn resolve_session(&self, id_or_short: &str) -> Result<Session, ManagerError> {
        let needle = id_or_short.to_string();
        let exact = SessionId::from_string(needle.clone());
        if let Some(s) = self
            .store
            .call(move |db| db.sessions_get(&exact))
            .await
            .map_err(ManagerError::internal)?
        {
            return Ok(s);
        }

        let all = self
            .store
            .call(|db| db.sessions_list(None, None))
            .await
            .map_err(ManagerError::internal)?;
        let matches: Vec<Session> = all
            .into_iter()
            .filter(|s| s.id.short() == needle || s.id.as_str().starts_with(&needle))
            .collect();

        match matches.len() {
            1 => Ok(matches.into_iter().next().ok_or_else(|| {
                ManagerError::rpc(err_code::SESSION_NOT_FOUND, "session not found")
            })?),
            0 => Err(ManagerError::rpc(
                err_code::SESSION_NOT_FOUND,
                format!("session not found: {id_or_short}"),
            )),
            _ => Err(ManagerError::rpc(
                err_code::INVALID_REQUEST,
                format!("ambiguous session id prefix: {id_or_short}"),
            )),
        }
    }

    pub async fn session_list(
        &self,
        project_name: Option<&str>,
        status: Option<SessionStatus>,
    ) -> Result<Vec<Session>, ManagerError> {
        let project_id = match project_name {
            Some(name) => {
                let n = name.to_string();
                let proj = self
                    .store
                    .call(move |db| db.projects_get_by_name(&n))
                    .await
                    .map_err(ManagerError::internal)?
                    .ok_or_else(|| {
                        ManagerError::rpc(
                            err_code::PROJECT_NOT_FOUND,
                            format!("project not found: {name}"),
                        )
                    })?;
                Some(proj.id)
            }
            None => None,
        };

        self.store
            .call(move |db| db.sessions_list(project_id.as_ref(), status))
            .await
            .map_err(ManagerError::internal)
    }

    fn get_live(&self, id: &SessionId) -> Result<Arc<PtySession>, ManagerError> {
        let guard = self
            .live
            .lock()
            .map_err(|_| ManagerError::internal("live session lock poisoned"))?;
        guard.get(id).cloned().ok_or_else(|| {
            ManagerError::rpc(
                err_code::INVALID_STATE_TRANSITION,
                format!("session is not live: {id}"),
            )
        })
    }

    pub async fn session_pause(&self, id_or_short: &str) -> Result<Session, ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        if !can_transition(sess.status, SessionStatus::Paused) {
            return Err(ManagerError::rpc(
                err_code::INVALID_STATE_TRANSITION,
                format!("cannot pause session in status {}", sess.status),
            ));
        }
        let pty = self.get_live(&sess.id)?;
        pty.pause().map_err(ManagerError::internal)?;

        let sid = sess.id.clone();
        let pid = sess.pid;
        self.store
            .call(move |db| db.sessions_update_status(&sid, SessionStatus::Paused, pid, None, None))
            .await
            .map_err(ManagerError::internal)?;

        let _ = self.events.send(Event::SessionStatus {
            session_id: sess.id.to_string(),
            from: sess.status,
            to: SessionStatus::Paused,
            reason: Some("user_pause".to_string()),
        });
        self.resolve_session(sess.id.as_str()).await
    }

    pub async fn session_resume(&self, id_or_short: &str) -> Result<Session, ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        if sess.status != SessionStatus::Paused
            || !can_transition(sess.status, SessionStatus::Running)
        {
            return Err(ManagerError::rpc(
                err_code::INVALID_STATE_TRANSITION,
                format!("cannot resume session in status {}", sess.status),
            ));
        }
        let pty = self.get_live(&sess.id)?;
        pty.resume().map_err(ManagerError::internal)?;

        let sid = sess.id.clone();
        let pid = sess.pid;
        self.store
            .call(move |db| {
                db.sessions_update_status(&sid, SessionStatus::Running, pid, None, None)
            })
            .await
            .map_err(ManagerError::internal)?;

        let _ = self.events.send(Event::SessionStatus {
            session_id: sess.id.to_string(),
            from: sess.status,
            to: SessionStatus::Running,
            reason: Some("user_resume".to_string()),
        });
        self.resolve_session(sess.id.as_str()).await
    }

    pub async fn session_kill(&self, id_or_short: &str) -> Result<Session, ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        if !can_transition(sess.status, SessionStatus::Failed) {
            return Err(ManagerError::rpc(
                err_code::INVALID_STATE_TRANSITION,
                format!("cannot kill session in status {}", sess.status),
            ));
        }

        let maybe_pty = {
            let mut guard = self
                .live
                .lock()
                .map_err(|_| ManagerError::internal("live session lock poisoned"))?;
            guard.remove(&sess.id)
        };

        if let Some(pty) = maybe_pty {
            let grace = Duration::from_millis(self.cfg.kill_grace_ms);
            tokio::task::spawn_blocking(move || pty.terminate(grace))
                .await
                .map_err(ManagerError::internal)?
                .map_err(ManagerError::internal)?;
        }

        let sid = sess.id.clone();
        let now = now_epoch_ms();
        self.store
            .call(move |db| {
                db.sessions_update_status(&sid, SessionStatus::Failed, None, None, Some(now))
            })
            .await
            .map_err(ManagerError::internal)?;

        let _ = self.events.send(Event::SessionStatus {
            session_id: sess.id.to_string(),
            from: sess.status,
            to: SessionStatus::Failed,
            reason: Some("user_kill".to_string()),
        });
        self.resolve_session(sess.id.as_str()).await
    }

    pub async fn session_input(&self, id_or_short: &str, data: &[u8]) -> Result<(), ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        let pty = self.get_live(&sess.id)?;
        pty.write_input(data).map_err(ManagerError::internal)
    }

    pub async fn session_resize(
        &self,
        id_or_short: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        let pty = self.get_live(&sess.id)?;
        pty.resize(cols, rows).map_err(ManagerError::internal)
    }

    pub async fn session_screen(&self, id_or_short: &str) -> Result<String, ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        let pty = self.get_live(&sess.id)?;
        pty.screen_text().map_err(ManagerError::internal)
    }

    pub async fn session_attach_handle(
        &self,
        id_or_short: &str,
    ) -> Result<(SessionId, Arc<PtySession>), ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        let pty = self.get_live(&sess.id)?;
        Ok((sess.id, pty))
    }

    pub async fn session_diff(
        &self,
        id_or_short: &str,
        opts: DiffOptions,
    ) -> Result<DiffPage, ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        let pid = sess.project_id.clone();
        let proj = self
            .store
            .call(move |db| db.projects_get(&pid))
            .await
            .map_err(ManagerError::internal)?
            .ok_or_else(|| ManagerError::rpc(err_code::PROJECT_NOT_FOUND, "project not found"))?;

        let wt = PathBuf::from(&sess.worktree_path);
        let base = proj.default_branch;
        tokio::task::spawn_blocking(move || latch_git::diff(&wt, &base, &opts))
            .await
            .map_err(ManagerError::internal)?
            .map_err(|e| ManagerError::rpc(err_code::INVALID_REQUEST, e.to_string()))
    }

    pub async fn session_merge_check(
        &self,
        id_or_short: &str,
    ) -> Result<MergeCheckResult, ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        let pid = sess.project_id.clone();
        let proj = self
            .store
            .call(move |db| db.projects_get(&pid))
            .await
            .map_err(ManagerError::internal)?
            .ok_or_else(|| ManagerError::rpc(err_code::PROJECT_NOT_FOUND, "project not found"))?;

        let repo = PathBuf::from(&proj.repo_path);
        let target = proj.default_branch;
        let branch = sess.branch_name;
        tokio::task::spawn_blocking(move || latch_git::merge_check(&repo, &target, &branch))
            .await
            .map_err(ManagerError::internal)?
            .map_err(|e| ManagerError::rpc(err_code::INVALID_REQUEST, e.to_string()))
    }

    pub async fn session_merge(
        &self,
        id_or_short: &str,
        strategy: MergeStrategy,
        message: Option<String>,
    ) -> Result<String, ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        if !sess.status.is_terminal() {
            return Err(ManagerError::rpc(
                err_code::INVALID_STATE_TRANSITION,
                "cannot merge while session is not terminal",
            ));
        }

        let pid = sess.project_id.clone();
        let proj = self
            .store
            .call(move |db| db.projects_get(&pid))
            .await
            .map_err(ManagerError::internal)?
            .ok_or_else(|| ManagerError::rpc(err_code::PROJECT_NOT_FOUND, "project not found"))?;

        let repo = PathBuf::from(&proj.repo_path);
        let wt = PathBuf::from(&sess.worktree_path);
        let target = proj.default_branch;
        let branch = sess.branch_name.clone();
        let msg = message.unwrap_or_else(|| format!("latch: merge {}", sess.id.short()));
        let managed_root = self.paths.worktrees_dir();

        let commit_oid = tokio::task::spawn_blocking(move || -> Result<String, GitError> {
            latch_git::commit_pending(&wt, &msg)?;
            let oid = latch_git::merge(&repo, &target, &branch, strategy, &msg)?;
            latch_git::remove_worktree(&repo, &wt, &branch, Some(&managed_root))?;
            Ok(oid)
        })
        .await
        .map_err(ManagerError::internal)?
        .map_err(|e| match e {
            GitError::Conflict(files) => ManagerError::Rpc {
                code: err_code::MERGE_CONFLICT,
                message: "merge conflict".to_string(),
                details: Some(json!({ "conflicts": files })),
            },
            other => ManagerError::rpc(err_code::INVALID_REQUEST, other.to_string()),
        })?;

        let sid = sess.id.clone();
        let now = now_epoch_ms();
        let oid_clone = commit_oid.clone();
        self.store
            .call(move |db| {
                db.sessions_mark_cleaned(&sid, now)?;
                db.audit_insert(Some(&sid), "merge", &format!("merged commit {oid_clone}"))
            })
            .await
            .map_err(ManagerError::internal)?;

        Ok(commit_oid)
    }

    pub async fn session_discard(&self, id_or_short: &str) -> Result<(), ManagerError> {
        let sess = self.resolve_session(id_or_short).await?;
        if !sess.status.is_terminal() {
            return Err(ManagerError::rpc(
                err_code::INVALID_STATE_TRANSITION,
                "cannot discard while session is not terminal",
            ));
        }

        let pid = sess.project_id.clone();
        let proj = self
            .store
            .call(move |db| db.projects_get(&pid))
            .await
            .map_err(ManagerError::internal)?
            .ok_or_else(|| ManagerError::rpc(err_code::PROJECT_NOT_FOUND, "project not found"))?;

        let repo = PathBuf::from(&proj.repo_path);
        let wt = PathBuf::from(&sess.worktree_path);
        let branch = sess.branch_name.clone();
        let managed_root = self.paths.worktrees_dir();

        tokio::task::spawn_blocking(move || {
            latch_git::remove_worktree(&repo, &wt, &branch, Some(&managed_root))
        })
        .await
        .map_err(ManagerError::internal)?
        .map_err(|e| ManagerError::rpc(err_code::INVALID_REQUEST, e.to_string()))?;

        let sid = sess.id.clone();
        let now = now_epoch_ms();
        self.store
            .call(move |db| {
                db.sessions_mark_cleaned(&sid, now)?;
                db.audit_insert(Some(&sid), "discard", "discarded worktree and branch")
            })
            .await
            .map_err(ManagerError::internal)?;

        Ok(())
    }

    pub async fn shutdown_all(&self) {
        let active: Vec<(SessionId, Arc<PtySession>)> = {
            let Ok(mut guard) = self.live.lock() else {
                return;
            };
            guard.drain().collect()
        };

        let grace = Duration::from_millis(self.cfg.kill_grace_ms);
        let now = now_epoch_ms();
        for (sid, pty) in active {
            let _ = tokio::task::spawn_blocking(move || pty.terminate(grace)).await;
            let _ = self
                .store
                .call(move |db| {
                    db.sessions_update_status(&sid, SessionStatus::Failed, None, None, Some(now))
                })
                .await;
        }
    }
}

fn build_clean_env(
    pass_env: &[String],
    profile_env: &std::collections::BTreeMap<String, String>,
) -> Vec<(String, String)> {
    const BASE_KEYS: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "TZ",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
    ];
    let mut map: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for key in BASE_KEYS {
        if let Ok(val) = std::env::var(key) {
            map.insert((*key).to_string(), val);
        }
    }
    map.insert("TERM".to_string(), "xterm-256color".to_string());
    for key in pass_env {
        if let Ok(val) = std::env::var(key) {
            map.insert(key.clone(), val);
        }
    }
    for (k, v) in profile_env {
        map.insert(k.clone(), v.clone());
    }
    map.into_iter().collect()
}
