#![allow(clippy::unwrap_used, clippy::expect_used)]

use latch_git::{
    DiffOptions, FileChangeKind, GitError, MergeCheckResult, MergeStrategy, changed_files,
    commit_pending, create_worktree, diff, ensure_min_version, list_worktrees, merge, merge_check,
    remove_worktree, repo_toplevel,
};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
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
}

fn init_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    fs::write(dir.path().join("README.md"), "line1\nline2\nline3\n").unwrap();
    git(dir.path(), &["add", "README.md"]);
    git(dir.path(), &["commit", "-m", "initial"]);
    dir
}

#[test]
fn version_and_non_repo_checks() {
    let ver = ensure_min_version().unwrap();
    assert!((ver.0, ver.1) >= (2, 38));

    let tmp = TempDir::new().unwrap();
    let err = repo_toplevel(tmp.path()).unwrap_err();
    assert!(matches!(err, GitError::NotARepo(_)));
}

#[test]
fn three_parallel_worktrees_and_idempotent_cleanup() {
    let repo = init_repo();
    let wt_root = TempDir::new().unwrap();

    for i in 1..=3 {
        let wt = wt_root.path().join(format!("s{i}"));
        let branch = format!("latch/demo/0000000{i}");
        create_worktree(repo.path(), &wt, &branch, "main").unwrap();
        assert!(wt.join("README.md").exists());
    }

    let list = list_worktrees(repo.path()).unwrap();
    assert_eq!(list.len(), 4);

    let wt1 = wt_root.path().join("s1");
    remove_worktree(
        repo.path(),
        &wt1,
        "latch/demo/00000001",
        Some(wt_root.path()),
    )
    .unwrap();
    // Cleanup kedua kali tetap sukses (idempoten).
    remove_worktree(
        repo.path(),
        &wt1,
        "latch/demo/00000001",
        Some(wt_root.path()),
    )
    .unwrap();
    assert!(!wt1.exists());
}

#[test]
fn diff_detects_modified_untracked_and_paginates() {
    let repo = init_repo();
    let wt_root = TempDir::new().unwrap();
    let wt = wt_root.path().join("s1");
    create_worktree(repo.path(), &wt, "latch/demo/diff1", "main").unwrap();

    fs::write(wt.join("README.md"), "line1\nchanged\nline3\n").unwrap();
    fs::write(wt.join("new_file.txt"), "hello world\n").unwrap();

    let files = changed_files(&wt, "main").unwrap();
    assert_eq!(files.len(), 2);
    assert!(
        files
            .iter()
            .any(|f| f.path == "README.md" && f.kind == FileChangeKind::Modified)
    );
    assert!(
        files
            .iter()
            .any(|f| f.path == "new_file.txt" && f.kind == FileChangeKind::Added)
    );

    let page1 = diff(
        &wt,
        "main",
        &DiffOptions {
            file: None,
            offset: 0,
            limit: 1,
            max_bytes: 256 * 1024,
        },
    )
    .unwrap();
    assert_eq!(page1.total, 2);
    assert_eq!(page1.files.len(), 1);
    assert!(!page1.patch.is_empty());

    let tiny = diff(
        &wt,
        "main",
        &DiffOptions {
            file: None,
            offset: 0,
            limit: 10,
            max_bytes: 20,
        },
    )
    .unwrap();
    assert!(tiny.truncated);
    assert!(tiny.patch.len() <= 20);
}

#[test]
fn merge_check_clean_and_conflict_and_squash_merge() {
    let repo = init_repo();
    let wt_root = TempDir::new().unwrap();

    let wt_a = wt_root.path().join("a");
    let wt_b = wt_root.path().join("b");
    create_worktree(repo.path(), &wt_a, "latch/demo/a", "main").unwrap();
    create_worktree(repo.path(), &wt_b, "latch/demo/b", "main").unwrap();

    fs::write(wt_a.join("README.md"), "from A\nline2\nline3\n").unwrap();
    fs::write(wt_a.join("extra.txt"), "extra from A\n").unwrap();
    assert!(commit_pending(&wt_a, "commit A").unwrap());

    fs::write(wt_b.join("README.md"), "from B\nline2\nline3\n").unwrap();
    assert!(commit_pending(&wt_b, "commit B").unwrap());

    // Merge A ke main bersih.
    let res_a = merge_check(repo.path(), "main", "latch/demo/a").unwrap();
    assert!(matches!(res_a, MergeCheckResult::Clean { .. }));
    merge(
        repo.path(),
        "main",
        "latch/demo/a",
        MergeStrategy::Squash,
        "merge A",
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(repo.path().join("extra.txt")).unwrap(),
        "extra from A\n"
    );

    // Sekarang B berkonflik dengan main pada README.md tanpa mengubah working tree.
    let res_b = merge_check(repo.path(), "main", "latch/demo/b").unwrap();
    match res_b {
        MergeCheckResult::Conflicts(files) => {
            assert!(files.iter().any(|f| f == "README.md"));
        }
        MergeCheckResult::Clean { .. } => panic!("expected conflict"),
    }
    let err = merge(
        repo.path(),
        "main",
        "latch/demo/b",
        MergeStrategy::Squash,
        "merge B",
    )
    .unwrap_err();
    assert!(matches!(err, GitError::Conflict(_)));
}
