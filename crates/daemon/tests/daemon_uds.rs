#![allow(clippy::unwrap_used, clippy::expect_used)]

use latch_core::client::Client;
use latch_core::config::{GlobalConfig, Paths};
use latch_core::model::{Project, Session, SessionStatus};
use latch_daemon::{DaemonConfig, run};
use serde_json::json;
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
        assert!(out.status.success());
    };
    run_git(&["init", "-b", "main"]);
    run_git(&["config", "user.name", "Test"]);
    run_git(&["config", "user.email", "test@example.com"]);
    fs::write(dir.join("README.md"), "init\n").unwrap();
    run_git(&["add", "README.md"]);
    run_git(&["commit", "-m", "init"]);
}

#[tokio::test]
async fn daemon_uds_lifecycle_and_session_execution() {
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
[profiles.echo]
command = "sh"
args = ["-c", "echo hi > out.txt; sleep 0.1"]
proxy_mode = "bypass"
"#,
    )
    .unwrap();

    let socket_path = state_dir.join("daemon.sock");
    let paths = Paths {
        config_dir: cfg_dir,
        state_dir,
        socket_path: socket_path.clone(),
    };
    let cfg = DaemonConfig {
        paths,
        global: GlobalConfig::default(),
    };

    let (tx, rx) = oneshot::channel::<()>();
    let server_handle = tokio::spawn(async move {
        run(cfg, async {
            let _ = rx.await;
        })
        .await
        .unwrap();
    });

    for _ in 0..40 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(socket_path.exists());

    let sock_clone = socket_path.clone();
    let repo_str = repo_dir.to_string_lossy().into_owned();
    let sess: Session = tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(&sock_clone).unwrap();
        let _proj: Project = client
            .call(
                "project.add",
                json!({ "name": "demo", "repo_path": repo_str }),
            )
            .unwrap();
        client
            .call(
                "session.create",
                json!({ "project": "demo", "profile": "echo", "task": "write file" }),
            )
            .unwrap()
    })
    .await
    .unwrap();

    // Tunggu sesi selesai.
    let mut final_status = sess.status;
    for _ in 0..40 {
        let sock = socket_path.clone();
        let sid = sess.id.to_string();
        let got: Session = tokio::task::spawn_blocking(move || {
            let mut client = Client::connect(&sock).unwrap();
            client.call("session.get", json!({ "id": sid })).unwrap()
        })
        .await
        .unwrap();
        final_status = got.status;
        if final_status.is_terminal() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(final_status, SessionStatus::Completed);

    // Uji attach: stream harus mengirim Snapshot dan berhenti saat proses keluar.
    let sock_attach = socket_path.clone();
    let sess2: Session = tokio::task::spawn_blocking(move || {
        let mut client = Client::connect(&sock_attach).unwrap();
        client
            .call(
                "session.create",
                json!({ "project": "demo", "profile": "echo", "task": "attach test" }),
            )
            .unwrap()
    })
    .await
    .unwrap();

    let sock_stream = socket_path.clone();
    let sid2 = sess2.id.to_string();
    let events = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || {
            let client = Client::connect(&sock_stream).unwrap();
            let stream = client.attach(&sid2).unwrap();
            stream.collect::<Result<Vec<_>, _>>().unwrap()
        }),
    )
    .await
    .expect("attach stream timed out")
    .unwrap();

    assert!(
        events
            .iter()
            .any(|e| matches!(e, latch_core::client::AttachEvent::Snapshot(_))),
        "expected Snapshot event in attach stream"
    );

    let _ = tx.send(());
    server_handle.await.unwrap();
}
