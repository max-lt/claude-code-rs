use std::path::Path;

use anyhow::Result;
use git2::StatusOptions;

use crate::repo::open_repo;

/// Possible states of a file in the working tree / index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    New,
    Modified,
    Deleted,
    Renamed,
    Typechange,
    Conflicted,
}

/// A single file entry from `git status`.
#[derive(Debug, Clone)]
pub struct StatusEntry {
    /// Path relative to the repo root.
    pub path: String,
    /// Status in the index (staged).
    pub index: Option<FileStatus>,
    /// Status in the working tree (unstaged).
    pub worktree: Option<FileStatus>,
}

/// Return the status of all changed files (like `git status --porcelain`).
pub fn status(path: &Path) -> Result<Vec<StatusEntry>> {
    let repo = open_repo(path)?;

    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true);

    let statuses = repo.statuses(Some(&mut opts))?;
    let mut entries = Vec::with_capacity(statuses.len());

    for entry in statuses.iter() {
        let s = entry.status();
        let path = entry.path().unwrap_or("<non-utf8>").to_string();

        let index = index_status(s);
        let worktree = worktree_status(s);

        entries.push(StatusEntry {
            path,
            index,
            worktree,
        });
    }

    Ok(entries)
}

fn index_status(s: git2::Status) -> Option<FileStatus> {
    if s.intersects(git2::Status::INDEX_NEW) {
        Some(FileStatus::New)
    } else if s.intersects(git2::Status::INDEX_MODIFIED) {
        Some(FileStatus::Modified)
    } else if s.intersects(git2::Status::INDEX_DELETED) {
        Some(FileStatus::Deleted)
    } else if s.intersects(git2::Status::INDEX_RENAMED) {
        Some(FileStatus::Renamed)
    } else if s.intersects(git2::Status::INDEX_TYPECHANGE) {
        Some(FileStatus::Typechange)
    } else if s.intersects(git2::Status::CONFLICTED) {
        Some(FileStatus::Conflicted)
    } else {
        None
    }
}

fn worktree_status(s: git2::Status) -> Option<FileStatus> {
    if s.intersects(git2::Status::WT_NEW) {
        Some(FileStatus::New)
    } else if s.intersects(git2::Status::WT_MODIFIED) {
        Some(FileStatus::Modified)
    } else if s.intersects(git2::Status::WT_DELETED) {
        Some(FileStatus::Deleted)
    } else if s.intersects(git2::Status::WT_RENAMED) {
        Some(FileStatus::Renamed)
    } else if s.intersects(git2::Status::WT_TYPECHANGE) {
        Some(FileStatus::Typechange)
    } else {
        None
    }
}

impl std::fmt::Display for FileStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::New => write!(f, "A"),
            Self::Modified => write!(f, "M"),
            Self::Deleted => write!(f, "D"),
            Self::Renamed => write!(f, "R"),
            Self::Typechange => write!(f, "T"),
            Self::Conflicted => write!(f, "C"),
        }
    }
}

impl std::fmt::Display for StatusEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let idx = self
            .index
            .map_or(' ', |s| s.to_string().chars().next().unwrap_or(' '));
        let wt = self
            .worktree
            .map_or(' ', |s| s.to_string().chars().next().unwrap_or(' '));
        write!(f, "{idx}{wt} {}", self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_repo() -> (TempDir, git2::Repository) {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        {
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            let tree_id = repo.index().unwrap().write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
        }

        (dir, repo)
    }

    #[test]
    fn test_clean_status() {
        let (dir, _) = init_repo();
        let entries = status(dir.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_untracked_file() {
        let (dir, _) = init_repo();
        fs::write(dir.path().join("new.txt"), "hello").unwrap();

        let entries = status(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "new.txt");
        assert!(entries[0].index.is_none());
        assert_eq!(entries[0].worktree, Some(FileStatus::New));
    }

    #[test]
    fn test_staged_file() {
        let (dir, repo) = init_repo();
        let file_path = dir.path().join("staged.txt");
        fs::write(&file_path, "staged content").unwrap();

        let mut index = repo.index().unwrap();
        index.add_path(Path::new("staged.txt")).unwrap();
        index.write().unwrap();

        let entries = status(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].index, Some(FileStatus::New));
    }

    #[test]
    fn test_display() {
        let entry = StatusEntry {
            path: "src/main.rs".to_string(),
            index: Some(FileStatus::Modified),
            worktree: None,
        };
        assert_eq!(format!("{entry}"), "M  src/main.rs");
    }
}
