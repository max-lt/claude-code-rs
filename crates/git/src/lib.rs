//! Git operations via libgit2 — no CLI dependency.

mod blame;
mod diff;
pub(crate) mod log;
mod repo;
mod show;
mod status;
mod write;

pub use blame::{BlameLine, blame, blame_range};
pub use diff::{DiffEntry, DiffStat, diff_range, diff_staged, diff_unstaged};
pub use log::{LogEntry, log as git_log};
pub use repo::{BranchInfo, current_branch, list_branches, open_repo, repo_root};
pub use show::{CommitDetail, show};
pub use status::{FileStatus, StatusEntry, status};
pub use write::{
    ResetMode, add, checkout, commit, create_branch, delete_branch, push, reset, unstage,
};
