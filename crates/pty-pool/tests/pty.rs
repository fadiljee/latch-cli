#![allow(clippy::unwrap_used, clippy::expect_used)]

use latch_core::SpawnSpec;
use latch_pty::{PtyOptions, PtySession, PtySize};
use nix::unistd::{Pid, getpgid};
use std::fs;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn sh_spec(script: &str) -> SpawnSpec {
    SpawnSpec {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        env: vec![
            (
                "PATH".to_string(),
                std::env::var("PATH").unwrap_or_default(),
            ),
            ("TERM".to_string(), "xterm-256color".to_string()),
        ],
        cwd: std::env::temp_dir(),
    }
}

#[tokio::test]
async fn spawn_captures_output_exit_code_and_is_session_leader() {
    let (sess, watcher) = PtySession::spawn(
        sh_spec("printf hello; exit 3"),
        PtySize::default(),
        PtyOptions::default(),
    )
    .unwrap();

    let pid = sess.pid();
    let pgid = getpgid(Some(Pid::from_raw(pid as i32))).unwrap();
    assert_eq!(
        pgid.as_raw(),
        pid as i32,
        "child must be its own PGID via setsid"
    );

    let exit = tokio::time::timeout(Duration::from_secs(5), watcher)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit.code, 3);
    assert!(!exit.success);

    tokio::time::sleep(Duration::from_millis(40)).await;
    let text = sess.screen_text().unwrap();
    assert!(
        text.contains("hello"),
        "expected 'hello' in screen: {text:?}"
    );
}

#[tokio::test]
async fn pause_resume_and_terminate_escalates_to_sigkill() {
    let (sess, watcher) = PtySession::spawn(
        sh_spec("trap '' TERM; sleep 100"),
        PtySize::default(),
        PtyOptions::default(),
    )
    .unwrap();
    let pid = sess.pid();
    tokio::time::sleep(Duration::from_millis(50)).await;

    sess.pause().unwrap();
    let mut paused = false;
    for _ in 0..50 {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        if stat.contains(") T ") {
            paused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(paused, "expected stopped state T within 500ms");

    sess.resume().unwrap();
    let mut resumed = false;
    for _ in 0..50 {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        if !stat.contains(") T ") {
            resumed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(resumed, "expected resumed state within 500ms");

    tokio::task::spawn_blocking(move || sess.terminate(Duration::from_millis(150)))
        .await
        .unwrap()
        .unwrap();

    let exit = tokio::time::timeout(Duration::from_secs(3), watcher)
        .await
        .unwrap()
        .unwrap();
    assert!(!exit.success);
}

#[tokio::test]
async fn ring_buffer_bounded_log_rotates_and_batching_interval() {
    let dir = TempDir::new().unwrap();
    let log_path = dir.path().join("pty.log");
    let opts = PtyOptions {
        ring_bytes: 4096,
        log_path: Some(log_path.clone()),
        log_max_bytes: 1024,
        log_max_files: 3,
    };

    let (sess, watcher) = PtySession::spawn(
        sh_spec("i=0; while [ $i -lt 400 ]; do printf '0123456789abcdef0123456789abcdef\\n'; i=$((i+1)); sleep 0.001; done"),
        PtySize::default(),
        opts,
    )
    .unwrap();

    let (_snap, mut rx) = sess.subscribe().unwrap();
    let mut times = Vec::new();
    let collect = async {
        while let Ok(frame) = rx.recv().await {
            if !frame.is_empty() {
                times.push(Instant::now());
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(collect, async {
            let _ = watcher.await;
        })
    })
    .await;

    let ring = sess.ring_snapshot().unwrap();
    assert!(ring.len() <= 4096, "ring exceeded cap: {}", ring.len());
    assert!(log_path.exists());
    assert!(
        dir.path().join("pty.log.1").exists(),
        "expected rotated log"
    );

    if times.len() >= 4 {
        let mut deltas: Vec<u128> = times
            .windows(2)
            .map(|w| w[1].duration_since(w[0]).as_millis())
            .collect();
        deltas.sort_unstable();
        let median = deltas[deltas.len() / 2];
        assert!(
            (8..=25).contains(&median),
            "expected median batching interval ~12ms, got {median}ms (deltas: {deltas:?})"
        );
    }
}

#[tokio::test]
async fn resize_updates_pty_and_snapshot() {
    let (sess, watcher) = PtySession::spawn(
        sh_spec("sleep 0.2; stty size"),
        PtySize { cols: 80, rows: 24 },
        PtyOptions::default(),
    )
    .unwrap();

    sess.resize(100, 35).unwrap();
    let exit = tokio::time::timeout(Duration::from_secs(5), watcher)
        .await
        .unwrap()
        .unwrap();
    assert!(exit.success);

    tokio::time::sleep(Duration::from_millis(40)).await;
    let text = sess.screen_text().unwrap();
    assert!(
        text.contains("35 100"),
        "expected '35 100' from stty size, got: {text:?}"
    );
    let (snap, _) = sess.subscribe().unwrap();
    assert!(!snap.is_empty());
}
