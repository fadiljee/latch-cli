use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use thiserror::Error;

const MIN_GIT_VERSION: (u32, u32) = (2, 38);
const DEFAULT_DIFF_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("git I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("git >= 2.38 required, found {found}")]
    VersionTooOld { found: String },

    #[error("not a git repository: {0}")]
    NotARepo(PathBuf),

    #[error("git command failed ({args:?}): {stderr}")]
    Command { args: Vec<String>, stderr: String },

    #[error("merge conflict in files: {0:?}")]
    Conflict(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub prunable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub kind: FileChangeKind,
    pub path: String,
    pub old_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiffOptions {
    pub file: Option<String>,
    pub offset: usize,
    pub limit: usize,
    pub max_bytes: usize,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            file: None,
            offset: 0,
            limit: 100,
            max_bytes: DEFAULT_DIFF_MAX_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffPage {
    pub files: Vec<ChangedFile>,
    pub patch: String,
    pub truncated: bool,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeCheckResult {
    Clean { tree: String },
    Conflicts(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    Squash,
    Merge,
}

fn git_cmd(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).arg("-c").arg("core.quotepath=off");
    cmd.args(args);
    cmd
}

fn run_git(dir: &Path, args: &[&str]) -> Result<Output, GitError> {
    let output = git_cmd(dir, args).output()?;
    if !output.status.success() {
        return Err(GitError::Command {
            args: args.iter().map(|s| (*s).to_string()).collect(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output)
}

/// Mengambil versi `git` terpasang sebagai `(major, minor, patch)`.
pub fn git_version() -> Result<(u32, u32, u32), GitError> {
    let output = Command::new("git").arg("--version").output()?;
    if !output.status.success() {
        return Err(GitError::Command {
            args: vec!["--version".to_string()],
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    parse_git_version(&raw).ok_or_else(|| GitError::VersionTooOld {
        found: raw.trim().to_string(),
    })
}

fn parse_git_version(raw: &str) -> Option<(u32, u32, u32)> {
    let token = raw
        .split_whitespace()
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = token.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next().unwrap_or("0").parse().ok()?;
    let patch: u32 = parts
        .next()
        .and_then(|p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(0);
    Some((major, minor, patch))
}

/// Memastikan `git` memenuhi versi minimum 2.38 (dibutuhkan untuk `merge-tree --write-tree`).
pub fn ensure_min_version() -> Result<(u32, u32, u32), GitError> {
    let ver = git_version()?;
    if (ver.0, ver.1) < MIN_GIT_VERSION {
        return Err(GitError::VersionTooOld {
            found: format!("{}.{}.{}", ver.0, ver.1, ver.2),
        });
    }
    Ok(ver)
}

/// Mengembalikan jalur root repositori Git untuk `path`.
pub fn repo_toplevel(path: &Path) -> Result<PathBuf, GitError> {
    let output = git_cmd(path, &["rev-parse", "--show-toplevel"]).output()?;
    if !output.status.success() {
        return Err(GitError::NotARepo(path.to_path_buf()));
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(s))
}

/// Mengembalikan nama branch aktif (`HEAD`) pada `repo`.
pub fn current_branch(repo: &Path) -> Result<String, GitError> {
    let out = run_git(repo, &["symbolic-ref", "--short", "HEAD"])?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Membuat worktree baru dengan branch baru dari `base_ref`.
pub fn create_worktree(
    repo: &Path,
    wt_path: &Path,
    branch: &str,
    base_ref: &str,
) -> Result<(), GitError> {
    if let Some(parent) = wt_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let wt_str = wt_path.to_string_lossy();
    run_git(repo, &["worktree", "add", "-b", branch, &wt_str, base_ref])?;
    Ok(())
}

/// Menghapus worktree dan branchnya secara idempoten.
/// Direktori sisa hanya dihapus jika berada di bawah `managed_root`.
pub fn remove_worktree(
    repo: &Path,
    wt_path: &Path,
    branch: &str,
    managed_root: Option<&Path>,
) -> Result<(), GitError> {
    if wt_path.exists() {
        let wt_str = wt_path.to_string_lossy();
        let _ = git_cmd(repo, &["worktree", "remove", "--force", &wt_str]).output();
    }
    let _ = git_cmd(repo, &["worktree", "prune"]).output();

    // Hapus sisa direktori hanya jika berada di bawah `managed_root`.
    if wt_path.exists()
        && let Some(root) = managed_root
        && let (Ok(canon_wt), Ok(canon_root)) = (wt_path.canonicalize(), root.canonicalize())
        && canon_wt.starts_with(&canon_root)
        && canon_wt != canon_root
    {
        fs::remove_dir_all(&canon_wt)?;
    }

    let check_branch = git_cmd(
        repo,
        &["show-ref", "--verify", &format!("refs/heads/{branch}")],
    )
    .output()?;
    if check_branch.status.success() {
        run_git(repo, &["branch", "-D", branch])?;
    }
    Ok(())
}

/// Membaca daftar worktree terdaftar dari `git worktree list --porcelain`.
pub fn list_worktrees(repo: &Path) -> Result<Vec<WorktreeInfo>, GitError> {
    let out = run_git(repo, &["worktree", "list", "--porcelain"])?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut result = Vec::new();
    let mut current: Option<WorktreeInfo> = None;

    for line in text.lines() {
        if line.is_empty() {
            if let Some(info) = current.take() {
                result.push(info);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(info) = current.take() {
                result.push(info);
            }
            current = Some(WorktreeInfo {
                path: PathBuf::from(rest),
                head: None,
                branch: None,
                bare: false,
                prunable: false,
            });
        } else if let Some(info) = current.as_mut() {
            if let Some(rest) = line.strip_prefix("HEAD ") {
                info.head = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("branch ") {
                let short = rest.strip_prefix("refs/heads/").unwrap_or(rest);
                info.branch = Some(short.to_string());
            } else if line == "bare" {
                info.bare = true;
            } else if line.starts_with("prunable") {
                info.prunable = true;
            }
        }
    }
    if let Some(info) = current {
        result.push(info);
    }
    Ok(result)
}

fn effective_merge_base(wt: &Path, base: &str) -> Result<String, GitError> {
    let out = run_git(wt, &["merge-base", base, "HEAD"])?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Mengembalikan daftar file yang berubah pada worktree relatif terhadap `base` (termasuk file untracked).
pub fn changed_files(wt: &Path, base: &str) -> Result<Vec<ChangedFile>, GitError> {
    let mb = effective_merge_base(wt, base)?;
    let diff_out = run_git(wt, &["diff", "--name-status", "-M", "-z", &mb])?;
    let mut files = parse_name_status_z(&diff_out.stdout);

    let untracked_out = run_git(wt, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for raw in untracked_out.stdout.split(|&b| b == 0) {
        if raw.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(raw).to_string();
        if !files.iter().any(|f| f.path == path) {
            files.push(ChangedFile {
                kind: FileChangeKind::Added,
                path,
                old_path: None,
            });
        }
    }
    Ok(files)
}

fn parse_name_status_z(bytes: &[u8]) -> Vec<ChangedFile> {
    let mut tokens = bytes
        .split(|&b| b == 0)
        .filter(|t| !t.is_empty())
        .map(|t| String::from_utf8_lossy(t).to_string());
    let mut out = Vec::new();
    while let Some(status) = tokens.next() {
        let code = status.chars().next().unwrap_or('M');
        match code {
            'R' | 'C' => {
                if let (Some(old_p), Some(new_p)) = (tokens.next(), tokens.next()) {
                    out.push(ChangedFile {
                        kind: if code == 'R' {
                            FileChangeKind::Renamed
                        } else {
                            FileChangeKind::Added
                        },
                        path: new_p,
                        old_path: Some(old_p),
                    });
                }
            }
            'A' => {
                if let Some(p) = tokens.next() {
                    out.push(ChangedFile {
                        kind: FileChangeKind::Added,
                        path: p,
                        old_path: None,
                    });
                }
            }
            'D' => {
                if let Some(p) = tokens.next() {
                    out.push(ChangedFile {
                        kind: FileChangeKind::Deleted,
                        path: p,
                        old_path: None,
                    });
                }
            }
            _ => {
                if let Some(p) = tokens.next() {
                    out.push(ChangedFile {
                        kind: FileChangeKind::Modified,
                        path: p,
                        old_path: None,
                    });
                }
            }
        }
    }
    out
}

/// Mengambil daftar perubahan dan patch unified dengan paginasi serta batas ukuran byte.
pub fn diff(wt: &Path, base: &str, opts: &DiffOptions) -> Result<DiffPage, GitError> {
    let all_files = changed_files(wt, base)?;
    let total = all_files.len();
    let mb = effective_merge_base(wt, base)?;

    let selected: Vec<ChangedFile> = if let Some(target_file) = &opts.file {
        all_files
            .into_iter()
            .filter(|f| &f.path == target_file)
            .collect()
    } else {
        let limit = if opts.limit == 0 {
            usize::MAX
        } else {
            opts.limit
        };
        all_files
            .into_iter()
            .skip(opts.offset)
            .take(limit)
            .collect()
    };

    let max_bytes = if opts.max_bytes == 0 {
        DEFAULT_DIFF_MAX_BYTES
    } else {
        opts.max_bytes
    };

    let untracked_out = run_git(wt, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let untracked: Vec<String> = untracked_out
        .stdout
        .split(|&b| b == 0)
        .filter(|t| !t.is_empty())
        .map(|t| String::from_utf8_lossy(t).to_string())
        .collect();

    let mut patch = String::new();
    let mut truncated = false;

    for f in &selected {
        let piece = if untracked.iter().any(|u| u == &f.path) {
            let out = git_cmd(
                wt,
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-index",
                    "--",
                    "/dev/null",
                    &f.path,
                ],
            )
            .output()?;
            // `git diff --no-index` mengembalikan kode keluar 1 saat ada perbedaan.
            if !out.status.success() && out.status.code() != Some(1) {
                return Err(GitError::Command {
                    args: vec!["diff --no-index".to_string(), f.path.clone()],
                    stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
                });
            }
            String::from_utf8_lossy(&out.stdout).to_string()
        } else {
            let out = run_git(
                wt,
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "-M",
                    &mb,
                    "--",
                    &f.path,
                ],
            )?;
            String::from_utf8_lossy(&out.stdout).to_string()
        };

        if patch.len() + piece.len() > max_bytes {
            let remaining = max_bytes.saturating_sub(patch.len());
            patch.push_str(truncate_utf8(&piece, remaining));
            truncated = true;
            break;
        }
        patch.push_str(&piece);
    }

    Ok(DiffPage {
        files: selected,
        patch,
        truncated,
        total,
    })
}

/// Menjalankan simulasi `git merge-tree --write-tree` tanpa mengubah working tree.
pub fn merge_check(repo: &Path, target: &str, branch: &str) -> Result<MergeCheckResult, GitError> {
    let out = git_cmd(
        repo,
        &[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            target,
            branch,
        ],
    )
    .output()?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    match out.status.code() {
        Some(0) => {
            let tree = stdout.lines().next().unwrap_or("").trim().to_string();
            Ok(MergeCheckResult::Clean { tree })
        }
        Some(1) => {
            let conflicts: Vec<String> = stdout
                .lines()
                .skip(1)
                .take_while(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .collect();
            Ok(MergeCheckResult::Conflicts(conflicts))
        }
        _ => Err(GitError::Command {
            args: vec![
                "merge-tree".to_string(),
                target.to_string(),
                branch.to_string(),
            ],
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        }),
    }
}

/// Melakukan commit atas perubahan tertunda di worktree. Mengembalikan `true` jika ada commit baru.
pub fn commit_pending(wt: &Path, message: &str) -> Result<bool, GitError> {
    let status = run_git(wt, &["status", "--porcelain"])?;
    if status.stdout.is_empty() {
        return Ok(false);
    }

    run_git(wt, &["add", "-A"])?;

    let email_out = git_cmd(wt, &["config", "user.email"]).output()?;
    let has_email = email_out.status.success() && !email_out.stdout.is_empty();

    let mut cmd = git_cmd(wt, &[]);
    if !has_email {
        cmd.arg("-c")
            .arg("user.name=latch")
            .arg("-c")
            .arg("user.email=latch@localhost");
    }
    cmd.args(["commit", "--no-verify", "-m", message]);
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(GitError::Command {
            args: vec!["commit".to_string()],
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(true)
}

/// Menggabungkan `branch` ke `target` menggunakan strategi squash atau merge.
pub fn merge(
    repo: &Path,
    target: &str,
    branch: &str,
    strategy: MergeStrategy,
    message: &str,
) -> Result<String, GitError> {
    let tree = match merge_check(repo, target, branch)? {
        MergeCheckResult::Clean { tree } => tree,
        MergeCheckResult::Conflicts(files) => return Err(GitError::Conflict(files)),
    };

    let target_oid = String::from_utf8_lossy(&run_git(repo, &["rev-parse", target])?.stdout)
        .trim()
        .to_string();
    let branch_oid = String::from_utf8_lossy(&run_git(repo, &["rev-parse", branch])?.stdout)
        .trim()
        .to_string();

    let mut commit_args = vec!["commit-tree", &tree, "-p", &target_oid];
    if strategy == MergeStrategy::Merge {
        commit_args.push("-p");
        commit_args.push(&branch_oid);
    }
    commit_args.push("-m");
    commit_args.push(message);

    let new_commit = String::from_utf8_lossy(&run_git(repo, &commit_args)?.stdout)
        .trim()
        .to_string();

    let head_branch = current_branch(repo).ok();
    if head_branch.as_deref() == Some(target) {
        run_git(repo, &["merge", "--ff-only", &new_commit])?;
    } else {
        run_git(
            repo,
            &[
                "update-ref",
                &format!("refs/heads/{target}"),
                &new_commit,
                &target_oid,
            ],
        )?;
    }

    Ok(new_commit)
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

    #[test]
    fn parse_version_handles_standard_and_apple_git() {
        assert_eq!(parse_git_version("git version 2.53.0"), Some((2, 53, 0)));
        assert_eq!(
            parse_git_version("git version 2.39.5 (Apple Git-154)"),
            Some((2, 39, 5))
        );
    }
}
