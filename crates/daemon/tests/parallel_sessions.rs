#![allow(clippy::unwrap_used, clippy::expect_used)]

use latch_core::client::Client;
use latch_core::config::{GlobalConfig, Paths};
use latch_core::model::{Project, ProxyMode, Session, SessionStatus};
use latch_core::{ProjectId, SessionId};
use latch_daemon::{DaemonConfig, run};
use latch_store::{Db, now_epoch_ms};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::oneshot;

fn init_repo(dir: &Path) {
    let run_git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run_git(&["init", "-b", "main"]);
    run_git(&["config", "user.name", "Test"]);
    run_git(&["config", "user.email", "test@example.com"]);
    fs::write(dir.join("README.md"), "base\n").unwrap();
    run_git(&["add", "README.md"]);
    run_git(&["commit", "-m", "init"]);
}

async fn wait_for_socket(sock: &Path) {
    for _ in 0..80 {
        if sock.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("socket not ready: {}", sock.display());
}

#[tokio::test]
async fn three_parallel_sessions_merge_discard_and_restart_recovery() {
    let root = TempDir::new().unwrap();
    let cfg_dir = root.path().join("cfg");
    let state_dir = root.path().join("state");
    let repo_dir = root.path().join("repo");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::create_dir_all(&repo_dir).unwrap();
    init_repo(&repo_dir);

    fs::write(
        cfg_dir.join("agents.toml"),
        r#"
[profiles.writer]
command = "sh"
args = ["-c", "echo \"$LATCH_SESSION_ID\" > out_$LATCH_SESSION_ID.txt; sleep 0.3"]
proxy_mode = "bypass"
"#,
    )
    .unwrap();

    let socket_path = state_dir.join("daemon.sock");
    let paths = Paths {
        config_dir: cfg_dir.clone(),
        state_dir: state_dir.clone(),
        socket_path: socket_path.clone(),
    };

    let (tx1, rx1) = oneshot::channel::<()>();
    let cfg1 = DaemonConfig {
        paths: paths.clone(),
        global: GlobalConfig::default(),
    };
    let h1 = tokio::spawn(async move {
        run(cfg1, async {
            let _ = rx1.await;
        })
        .await
        .unwrap();
    });
    wait_for_socket(&socket_path).await;

    let sock = socket_path.clone();
    let repo_str = repo_dir.to_string_lossy().into_owned();
    let created: Vec<Session> = tokio::task::spawn_blocking(move || {
        let mut c = Client::connect(&sock).unwrap();
        let _: Project = c
            .call(
                "project.add",
                json!({ "name": "demo", "repo_path": repo_str }),
            )
            .unwrap();
        let mut out = Vec::new();
        for i in 0..3 {
            let s: Session = c
                .call(
                    "session.create",
                    json!({ "project": "demo", "profile": "writer", "task": format!("t{i}") }),
                )
                .unwrap();
            out.push(s);
        }
        out
    })
    .await
    .unwrap();
    assert_eq!(created.len(), 3);

    // Tunggu ketiga sesi selesai.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let sock = socket_path.clone();
        let list: Vec<Session> = tokio::task::spawn_blocking(move || {
            let mut c = Client::connect(&sock).unwrap();
            c.call("session.list", json!({})).unwrap()
        })
        .await
        .unwrap();
        if list.iter().all(|s| s.status == SessionStatus::Completed) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "sessions did not complete in time: {list:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Verifikasi diff terpisah, merge sesi 0, discard sesi 1.
    let sock = socket_path.clone();
    let s0_id = created[0].id.to_string();
    let s1_id = created[1].id.to_string();
    let s2_id = created[2].id.to_string();
    tokio::task::spawn_blocking(move || {
        let mut c = Client::connect(&sock).unwrap();
        for sid in [&s0_id, &s1_id, &s2_id] {
            let d: Value = c.call("session.diff", json!({ "id": sid })).unwrap();
            let files = d["files"].as_array().unwrap();
            assert_eq!(files.len(), 1);
            assert_eq!(files[0]["path"].as_str().unwrap(), format!("out_{sid}.txt"));
        }
        let _: Value = c
            .call(
                "session.merge",
                json!({ "id": s0_id, "strategy": "squash" }),
            )
            .unwrap();
        let _: Value = c.call("session.discard", json!({ "id": s1_id })).unwrap();
    })
    .await
    .unwrap();

    assert!(repo_dir.join(format!("out_{}.txt", created[0].id)).exists());

    // Matikan daemon dan start ulang di direktori state yang sama -> 3 sesi tetap ada.
    let _ = tx1.send(());
    h1.await.unwrap();

    let (tx2, rx2) = oneshot::channel::<()>();
    let cfg2 = DaemonConfig {
        paths: paths.clone(),
        global: GlobalConfig::default(),
    };
    let h2 = tokio::spawn(async move {
        run(cfg2, async {
            let _ = rx2.await;
        })
        .await
        .unwrap();
    });
    wait_for_socket(&socket_path).await;

    let sock = socket_path.clone();
    let recovered: Vec<Session> = tokio::task::spawn_blocking(move || {
        let mut c = Client::connect(&sock).unwrap();
        c.call("session.list", json!({})).unwrap()
    })
    .await
    .unwrap();
    assert_eq!(recovered.len(), 3);
    assert!(
        recovered
            .iter()
            .all(|s| s.status == SessionStatus::Completed)
    );

    let _ = tx2.send(());
    h2.await.unwrap();
}

#[tokio::test]
async fn startup_reconciles_dead_pid_and_detects_orphan_worktree() {
    let root = TempDir::new().unwrap();
    let cfg_dir = root.path().join("cfg");
    let state_dir = root.path().join("state");
    let repo_dir = root.path().join("repo");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::create_dir_all(&repo_dir).unwrap();
    init_repo(&repo_dir);

    let socket_path = state_dir.join("daemon.sock");
    let paths = Paths {
        config_dir: cfg_dir,
        state_dir: state_dir.clone(),
        socket_path: socket_path.clone(),
    };

    // Siapkan DB dengan 1 proyek dan 1 sesi `running` ber-PID mati, serta 1 worktree yatim.
    let db = Db::open(&paths.db_path()).unwrap();
    let proj = Project {
        id: ProjectId::new(),
        name: "demo".to_string(),
        repo_path: repo_dir
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        default_branch: "main".to_string(),
        created_at_ms: now_epoch_ms(),
    };
    db.projects_insert(&proj).unwrap();

    let dead_sid = SessionId::new();
    db.sessions_insert(&Session {
        id: dead_sid.clone(),
        project_id: proj.id,
        agent_profile: "writer".to_string(),
        worktree_path: state_dir
            .join("worktrees/demo")
            .join(dead_sid.as_str())
            .to_string_lossy()
            .into_owned(),
        branch_name: format!("latch/demo/{}", dead_sid.short()),
        status: SessionStatus::Running,
        pid: Some(999_999),
        started_at_ms: now_epoch_ms(),
        ended_at_ms: None,
        task: None,
        exit_code: None,
        trip_reason: None,
        proxy_mode: ProxyMode::Bypass,
        sandbox_provider: "noop".to_string(),
        cleaned_at_ms: Some(now_epoch_ms()),
    })
    .unwrap();
    drop(db);

    let orphan_path = state_dir.join("worktrees/demo/orphan-wt");
    latch_git::create_worktree(&repo_dir, &orphan_path, "latch/demo/orphan", "main").unwrap();

    let (tx, rx) = oneshot::channel::<()>();
    let cfg = DaemonConfig {
        paths,
        global: GlobalConfig::default(),
    };
    let handle = tokio::spawn(async move {
        run(cfg, async {
            let _ = rx.await;
        })
        .await
        .unwrap();
    });
    wait_for_socket(&socket_path).await;

    let sock = socket_path.clone();
    let sid_str = dead_sid.to_string();
    let (sess, status_val): (Session, Value) = tokio::task::spawn_blocking(move || {
        let mut c = Client::connect(&sock).unwrap();
        let s: Session = c.call("session.get", json!({ "id": sid_str })).unwrap();
        let st: Value = c.call("daemon.status", json!({})).unwrap();
        (s, st)
    })
    .await
    .unwrap();

    assert_eq!(sess.status, SessionStatus::Failed);
    let orphans = status_val["orphan_worktrees"].as_array().unwrap();
    assert_eq!(orphans.len(), 1);
    assert!(orphans[0].as_str().unwrap().contains("orphan-wt"));

    let _ = tx.send(());
    handle.await.unwrap();
}
