#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn cli_help_and_config_check_invalid_exit_code() {
    let bin = env!("CARGO_BIN_EXE_latch-cli");

    let help = Command::new(bin).arg("--help").output().unwrap();
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("daemon"));
    assert!(stdout.contains("session"));

    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("agents.toml"),
        "[profiles.bad]\ncommand = \"\"\n",
    )
    .unwrap();

    let check = Command::new(bin)
        .args(["config", "check"])
        .env("LATCH_CONFIG_DIR", dir.path())
        .output()
        .unwrap();
    assert_eq!(check.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(stderr.contains("command"));
}
