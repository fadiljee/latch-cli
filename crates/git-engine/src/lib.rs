//! Mesin Git worktree, diff, simulasi merge-tree, dan squash/merge.

pub mod engine;

pub use engine::{
    ChangedFile, DiffOptions, DiffPage, FileChangeKind, GitError, MergeCheckResult, MergeStrategy,
    WorktreeInfo, changed_files, commit_pending, create_worktree, current_branch, diff,
    ensure_min_version, git_version, list_worktrees, merge, merge_check, remove_worktree,
    repo_toplevel,
};
