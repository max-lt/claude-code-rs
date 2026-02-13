use std::path::Path;

use anyhow::{Context, Result};
use git2::{DiffOptions, Repository};

use crate::repo::open_repo;

/// Summary statistics for a diff.
#[derive(Debug, Clone, Default)]
pub struct DiffStat {
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

/// A single file's diff output.
#[derive(Debug, Clone)]
pub struct DiffEntry {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub patch: String,
}

/// Show diff of staged changes (index vs HEAD), like `git diff --cached`.
pub fn diff_staged(path: &Path) -> Result<(Vec<DiffEntry>, DiffStat)> {
    let repo = open_repo(path)?;
    let head_tree = head_tree(&repo)?;

    let diff = repo
        .diff_tree_to_index(head_tree.as_ref(), None, Some(&mut diff_opts()))
        .context("failed to diff staged changes")?;

    collect_diff(&diff)
}

/// Show diff of unstaged changes (workdir vs index), like `git diff`.
pub fn diff_unstaged(path: &Path) -> Result<(Vec<DiffEntry>, DiffStat)> {
    let repo = open_repo(path)?;

    let diff = repo
        .diff_index_to_workdir(None, Some(&mut diff_opts()))
        .context("failed to diff unstaged changes")?;

    collect_diff(&diff)
}

/// Show diff between two revisions, like `git diff rev1..rev2`.
pub fn diff_range(path: &Path, from: &str, to: &str) -> Result<(Vec<DiffEntry>, DiffStat)> {
    let repo = open_repo(path)?;

    let from_obj = repo
        .revparse_single(from)
        .with_context(|| format!("cannot resolve revision: {from}"))?;
    let to_obj = repo
        .revparse_single(to)
        .with_context(|| format!("cannot resolve revision: {to}"))?;

    let from_tree = from_obj
        .peel_to_tree()
        .with_context(|| format!("{from} does not point to a tree"))?;
    let to_tree = to_obj
        .peel_to_tree()
        .with_context(|| format!("{to} does not point to a tree"))?;

    let diff = repo
        .diff_tree_to_tree(Some(&from_tree), Some(&to_tree), Some(&mut diff_opts()))
        .context("failed to compute diff")?;

    collect_diff(&diff)
}

// ── helpers ──────────────────────────────────────────────────────────────

fn diff_opts() -> DiffOptions {
    let mut opts = DiffOptions::new();
    opts.context_lines(3);
    opts
}

fn head_tree(repo: &Repository) -> Result<Option<git2::Tree<'_>>> {
    match repo.head() {
        Ok(head) => {
            let tree = head
                .peel_to_tree()
                .context("HEAD does not point to a tree")?;
            Ok(Some(tree))
        }
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn collect_diff(diff: &git2::Diff<'_>) -> Result<(Vec<DiffEntry>, DiffStat)> {
    let stats = diff.stats().context("failed to compute diff stats")?;
    let diff_stat = DiffStat {
        files_changed: stats.files_changed(),
        insertions: stats.insertions(),
        deletions: stats.deletions(),
    };

    let mut entries = Vec::new();

    for (i, delta) in diff.deltas().enumerate() {
        let old_path = delta.old_file().path().map(|p| p.display().to_string());
        let new_path = delta.new_file().path().map(|p| p.display().to_string());

        let patch = match git2::Patch::from_diff(diff, i)? {
            Some(mut patch) => {
                let buf = patch.to_buf()?;
                String::from_utf8_lossy(&buf).to_string()
            }
            None => String::new(),
        };

        entries.push(DiffEntry {
            old_path,
            new_path,
            patch,
        });
    }

    Ok((entries, diff_stat))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_repo_with_file() -> (TempDir, git2::Repository) {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();

        // Create and commit a file
        let file = dir.path().join("hello.txt");
        fs::write(&file, "hello world\n").unwrap();

        {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("hello.txt")).unwrap();
            index.write().unwrap();

            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let sig = git2::Signature::now("Test", "test@test.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
        }

        (dir, repo)
    }

    #[test]
    fn test_diff_staged_empty() {
        let (dir, _) = init_repo_with_file();
        let (entries, stat) = diff_staged(dir.path()).unwrap();
        assert!(entries.is_empty());
        assert_eq!(stat.files_changed, 0);
    }

    #[test]
    fn test_diff_staged_with_changes() {
        let (dir, repo) = init_repo_with_file();

        // Modify file and stage it
        fs::write(dir.path().join("hello.txt"), "hello world\nline 2\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("hello.txt")).unwrap();
        index.write().unwrap();

        let (entries, stat) = diff_staged(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(stat.files_changed, 1);
        assert_eq!(stat.insertions, 1);
        assert!(entries[0].patch.contains("+line 2"));
    }

    #[test]
    fn test_diff_unstaged() {
        let (dir, _) = init_repo_with_file();

        // Modify file without staging
        fs::write(dir.path().join("hello.txt"), "modified\n").unwrap();

        let (entries, stat) = diff_unstaged(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(stat.files_changed, 1);
        assert!(entries[0].patch.contains("+modified"));
    }
}
