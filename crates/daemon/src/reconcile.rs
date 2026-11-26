use latch_core::config::Paths;
use latch_core::model::SessionStatus;
use latch_store::{Store, now_epoch_ms};
use nix::errno::Errno;
use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::Pid;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Menjalankan rekonsiliasi startup untuk sesi yang tidak berhenti bersih dan mendeteksi worktree yatim.
pub async fn run(store: &Store, paths: &Paths) -> anyhow::Result<Vec<String>> {
    let sessions = store.call(|db| db.sessions_list(None, None)).await?;
    let now = now_epoch_ms();

    for sess in &sessions {
        let needs_reconcile = matches!(
            sess.status,
            SessionStatus::Running | SessionStatus::Paused | SessionStatus::WaitingApproval
        );
        if !needs_reconcile {
            continue;
        }

        if let Some(pid_u32) = sess.pid
            && let Ok(pid_raw) = i32::try_from(pid_u32)
        {
            let pid = Pid::from_raw(pid_raw);
            if kill(pid, None).is_ok() {
                match killpg(pid, Signal::SIGKILL) {
                    Ok(()) | Err(Errno::ESRCH) => {}
                    Err(err) => {
                        tracing::warn!(session_id = %sess.id, pid = pid_u32, error = %err, "failed to kill stale process group");
                    }
                }
                let sid = sess.id.clone();
                let _ = store
                    .call(move |db| {
                        db.audit_insert(
                            Some(&sid),
                            "reconcile_killed",
                            &format!("killed orphaned process group pid={pid_u32}"),
                        )
                    })
                    .await;
            }
        }

        let sid = sess.id.clone();
        store
            .call(move |db| {
                db.sessions_update_status(&sid, SessionStatus::Failed, None, None, Some(now))
            })
            .await?;
    }

    // Deteksi worktree yatim di bawah direktori worktrees yang dikelola.
    let active_worktrees: HashSet<PathBuf> = sessions
        .iter()
        .filter(|s| s.cleaned_at_ms.is_none())
        .map(|s| canonical_or_raw(Path::new(&s.worktree_path)))
        .collect();

    let projects = store.call(|db| db.projects_list()).await?;
    let managed_root = paths.worktrees_dir();
    let mut orphans = Vec::new();

    for proj in projects {
        let proj_wt_dir = canonical_or_raw(&managed_root.join(&proj.name));
        let repo_path = PathBuf::from(&proj.repo_path);
        let listed = tokio::task::spawn_blocking(move || latch_git::list_worktrees(&repo_path))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();

        for wt in listed {
            let canon_wt = canonical_or_raw(&wt.path);
            if canon_wt.starts_with(&proj_wt_dir) && !active_worktrees.contains(&canon_wt) {
                let path_str = wt.path.to_string_lossy().into_owned();
                tracing::warn!(project = %proj.name, worktree = %path_str, "detected orphan worktree");
                let msg = format!("orphan worktree detected for {}: {path_str}", proj.name);
                let _ = store
                    .call(move |db| db.audit_insert(None, "orphan_worktree", &msg))
                    .await;
                orphans.push(path_str);
            }
        }
    }

    Ok(orphans)
}

fn canonical_or_raw(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}
