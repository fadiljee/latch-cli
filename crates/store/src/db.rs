use latch_core::model::{Project, ProxyMode, Session, SessionStatus};
use latch_core::redact::redact;
use latch_core::{ProjectId, SessionId};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const AUDIT_DETAIL_MAX_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub id: i64,
    pub session_id: Option<String>,
    pub kind: String,
    pub detail: String,
    pub timestamp_ms: i64,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database integrity check failed: {0}")]
    IntegrityFailed(String),

    #[error("invalid enum value in database: {0}")]
    InvalidData(String),

    #[error("store actor channel closed")]
    ActorClosed,
}

/// Mengembalikan waktu UTC saat ini dalam milidetik sejak epoch Unix.
#[must_use]
pub fn now_epoch_ms() -> i64 {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(dur.as_millis()).unwrap_or(i64::MAX)
}

/// Koneksi SQLite sinkron yang dikelola oleh thread aktor `Store`.
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Membuka (atau membuat) database SQLite, mengaktifkan WAL, memeriksa integritas, dan menjalankan migrasi.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }

        let mut conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )?;

        let integrity: String = conn.query_row("PRAGMA integrity_check;", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(StoreError::IntegrityFailed(integrity));
        }

        run_migrations(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn projects_insert(&self, project: &Project) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO projects (id, name, repo_path, default_branch, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                project.id.as_str(),
                project.name,
                project.repo_path,
                project.default_branch,
                project.created_at_ms,
            ],
        )?;
        Ok(())
    }

    pub fn projects_list(&self) -> Result<Vec<Project>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, repo_path, default_branch, created_at
             FROM projects ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], row_to_project)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn projects_get(&self, id: &ProjectId) -> Result<Option<Project>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, name, repo_path, default_branch, created_at
                 FROM projects WHERE id = ?1",
                params![id.as_str()],
                row_to_project,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn projects_get_by_name(&self, name: &str) -> Result<Option<Project>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, name, repo_path, default_branch, created_at
                 FROM projects WHERE name = ?1",
                params![name],
                row_to_project,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn projects_delete(&self, id: &ProjectId) -> Result<bool, StoreError> {
        let affected = self
            .conn
            .execute("DELETE FROM projects WHERE id = ?1", params![id.as_str()])?;
        Ok(affected > 0)
    }

    pub fn sessions_insert(&self, s: &Session) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO sessions (
                id, project_id, agent_profile, worktree_path, branch_name,
                status, pid, started_at, ended_at, task,
                exit_code, trip_reason, proxy_mode, sandbox_provider, cleaned_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                s.id.as_str(),
                s.project_id.as_str(),
                s.agent_profile,
                s.worktree_path,
                s.branch_name,
                s.status.as_str(),
                s.pid.map(i64::from),
                s.started_at_ms,
                s.ended_at_ms,
                s.task,
                s.exit_code,
                s.trip_reason,
                s.proxy_mode.as_str(),
                s.sandbox_provider,
                s.cleaned_at_ms,
            ],
        )?;
        Ok(())
    }

    pub fn sessions_get(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, project_id, agent_profile, worktree_path, branch_name,
                        status, pid, started_at, ended_at, task,
                        exit_code, trip_reason, proxy_mode, sandbox_provider, cleaned_at
                 FROM sessions WHERE id = ?1",
                params![id.as_str()],
                row_to_session,
            )
            .optional()?
            .transpose()
    }

    pub fn sessions_list(
        &self,
        project_id: Option<&ProjectId>,
        status: Option<SessionStatus>,
    ) -> Result<Vec<Session>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, agent_profile, worktree_path, branch_name,
                    status, pid, started_at, ended_at, task,
                    exit_code, trip_reason, proxy_mode, sandbox_provider, cleaned_at
             FROM sessions
             WHERE (?1 IS NULL OR project_id = ?1)
               AND (?2 IS NULL OR status = ?2)
             ORDER BY started_at DESC",
        )?;
        let rows = stmt.query_map(
            params![
                project_id.map(ProjectId::as_str),
                status.map(SessionStatus::as_str)
            ],
            row_to_session,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    pub fn sessions_update_status(
        &self,
        id: &SessionId,
        status: SessionStatus,
        pid: Option<u32>,
        exit_code: Option<i32>,
        ended_at_ms: Option<i64>,
    ) -> Result<bool, StoreError> {
        let affected = self.conn.execute(
            "UPDATE sessions
             SET status = ?2, pid = ?3, exit_code = COALESCE(?4, exit_code), ended_at = COALESCE(?5, ended_at)
             WHERE id = ?1",
            params![
                id.as_str(),
                status.as_str(),
                pid.map(i64::from),
                exit_code,
                ended_at_ms,
            ],
        )?;
        Ok(affected > 0)
    }

    pub fn sessions_set_pid(&self, id: &SessionId, pid: Option<u32>) -> Result<bool, StoreError> {
        let affected = self.conn.execute(
            "UPDATE sessions SET pid = ?2 WHERE id = ?1",
            params![id.as_str(), pid.map(i64::from)],
        )?;
        Ok(affected > 0)
    }

    pub fn sessions_mark_cleaned(
        &self,
        id: &SessionId,
        cleaned_at_ms: i64,
    ) -> Result<bool, StoreError> {
        let affected = self.conn.execute(
            "UPDATE sessions SET cleaned_at = ?2 WHERE id = ?1",
            params![id.as_str(), cleaned_at_ms],
        )?;
        Ok(affected > 0)
    }

    pub fn audit_insert(
        &self,
        session_id: Option<&SessionId>,
        kind: &str,
        detail: &str,
    ) -> Result<(), StoreError> {
        let safe = redact(detail);
        let truncated = truncate_utf8(&safe, AUDIT_DETAIL_MAX_BYTES);
        self.conn.execute(
            "INSERT INTO audit_log (session_id, kind, detail, timestamp)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                session_id.map(SessionId::as_str),
                kind,
                truncated,
                now_epoch_ms()
            ],
        )?;
        Ok(())
    }

    pub fn audit_list(&self, limit: usize) -> Result<Vec<AuditEntry>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, kind, detail, timestamp
             FROM audit_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![i64::try_from(limit).unwrap_or(100)], |r| {
            Ok(AuditEntry {
                id: r.get(0)?,
                session_id: r.get(1)?,
                kind: r.get(2)?,
                detail: r.get(3)?,
                timestamp_ms: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

fn run_migrations(conn: &mut Connection) -> Result<(), StoreError> {
    let has_migrations_table: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
        [],
        |r| r.get(0),
    )?;

    let already_applied = if has_migrations_table {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 1)",
            [],
            |r| r.get::<_, bool>(0),
        )?
    } else {
        false
    };

    if !already_applied {
        let tx = conn.transaction()?;
        tx.execute_batch(MIGRATION_0001)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
            params![1_i64, "0001_init", now_epoch_ms()],
        )?;
        tx.commit()?;
    }

    Ok(())
}

fn row_to_project(r: &Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: ProjectId::from_string(r.get::<_, String>(0)?),
        name: r.get(1)?,
        repo_path: r.get(2)?,
        default_branch: r.get(3)?,
        created_at_ms: r.get(4)?,
    })
}

fn row_to_session(r: &Row<'_>) -> rusqlite::Result<Result<Session, StoreError>> {
    let id_str: String = r.get(0)?;
    let proj_str: String = r.get(1)?;
    let status_str: String = r.get(5)?;
    let pid_opt: Option<i64> = r.get(6)?;
    let proxy_str: String = r.get(12)?;

    let status = match status_str.parse::<SessionStatus>() {
        Ok(s) => s,
        Err(e) => return Ok(Err(StoreError::InvalidData(e.to_string()))),
    };
    let proxy_mode = match proxy_str.parse::<ProxyMode>() {
        Ok(p) => p,
        Err(e) => return Ok(Err(StoreError::InvalidData(e.to_string()))),
    };
    let pid = pid_opt.and_then(|v| u32::try_from(v).ok());

    Ok(Ok(Session {
        id: SessionId::from_string(id_str),
        project_id: ProjectId::from_string(proj_str),
        agent_profile: r.get(2)?,
        worktree_path: r.get(3)?,
        branch_name: r.get(4)?,
        status,
        pid,
        started_at_ms: r.get(7)?,
        ended_at_ms: r.get(8)?,
        task: r.get(9)?,
        exit_code: r.get(10)?,
        trip_reason: r.get(11)?,
        proxy_mode,
        sandbox_provider: r.get(13)?,
        cleaned_at_ms: r.get(14)?,
    }))
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_project() -> Project {
        Project {
            id: ProjectId::new(),
            name: "demo".to_string(),
            repo_path: "/tmp/demo".to_string(),
            default_branch: "main".to_string(),
            created_at_ms: now_epoch_ms(),
        }
    }

    fn sample_session(project_id: &ProjectId) -> Session {
        let id = SessionId::new();
        Session {
            id: id.clone(),
            project_id: project_id.clone(),
            agent_profile: "sleeper".to_string(),
            worktree_path: format!("/tmp/wt/{id}"),
            branch_name: format!("latch/demo/{}", id.short()),
            status: SessionStatus::Running,
            pid: Some(12345),
            started_at_ms: now_epoch_ms(),
            ended_at_ms: None,
            task: Some("test task".to_string()),
            exit_code: None,
            trip_reason: None,
            proxy_mode: ProxyMode::Bypass,
            sandbox_provider: "noop".to_string(),
            cleaned_at_ms: None,
        }
    }

    #[test]
    fn migration_is_idempotent_and_crud_works() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("latch.db");

        let db = Db::open(&db_path).unwrap();
        let proj = sample_project();
        db.projects_insert(&proj).unwrap();
        assert_eq!(db.projects_list().unwrap().len(), 1);

        let sess = sample_session(&proj.id);
        db.sessions_insert(&sess).unwrap();
        let fetched = db.sessions_get(&sess.id).unwrap().unwrap();
        assert_eq!(fetched.status, SessionStatus::Running);

        db.sessions_update_status(
            &sess.id,
            SessionStatus::Completed,
            None,
            Some(0),
            Some(now_epoch_ms()),
        )
        .unwrap();
        let updated = db.sessions_get(&sess.id).unwrap().unwrap();
        assert_eq!(updated.status, SessionStatus::Completed);
        assert_eq!(updated.exit_code, Some(0));

        db.audit_insert(Some(&sess.id), "test", "secret sk-1234567890abcdef1234")
            .unwrap();
        let logs = db.audit_list(10).unwrap();
        assert!(!logs[0].detail.contains("1234567890"));
        drop(db);

        // Buka ulang harus idempoten
        let db2 = Db::open(&db_path).unwrap();
        assert_eq!(db2.projects_list().unwrap().len(), 1);
    }

    #[test]
    fn check_constraint_rejects_invalid_status_and_no_secret_columns() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("latch.db");
        let db = Db::open(&db_path).unwrap();

        let proj = sample_project();
        db.projects_insert(&proj).unwrap();

        let err = db.conn.execute(
            "INSERT INTO sessions (id, project_id, agent_profile, worktree_path, branch_name, status, started_at)
             VALUES ('s1', ?1, 'p', '/tmp/w', 'b', 'bogus', 1)",
            params![proj.id.as_str()],
        );
        assert!(err.is_err());

        // Pastikan tidak ada kolom rahasia di seluruh skema
        let mut stmt = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for table in tables {
            let mut pragma = db
                .conn
                .prepare(&format!("PRAGMA table_info(\"{table}\")"))
                .unwrap();
            let cols: Vec<String> = pragma
                .query_map([], |r| r.get(1))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            for col in cols {
                let lower = col.to_ascii_lowercase();
                assert!(
                    !lower.contains("key") && !lower.contains("authorization"),
                    "forbidden secret column {table}.{col}"
                );
            }
        }
    }
}
