//! Git operations via libgit2 — no CLI dependency.

mod blame;
mod diff;
pub(crate) mod log;
mod repo;
mod show;
mod status;

pub use blame::{blame, blame_range, BlameLine};
pub use diff::{diff_staged, diff_unstaged, diff_range, DiffEntry, DiffStat};
pub use log::{log as git_log, LogEntry};
pub use repo::{open_repo, repo_root, current_branch, list_branches, BranchInfo};
pub use show::{show, CommitDetail};
pub use status::{status, StatusEntry, FileStatus};
