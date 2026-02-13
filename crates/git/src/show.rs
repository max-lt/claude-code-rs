use std::path::Path;

use anyhow::{Context, Result};

use crate::diff::{DiffEntry, DiffStat};
use crate::repo::open_repo;

/// Full details of a single commit.
#[derive(Debug, Clone)]
pub struct CommitDetail {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
    pub diff_entries: Vec<DiffEntry>,
    pub stat: DiffStat,
}

/// Show a single commit with its diff (like `git show <rev>`).
pub fn show(path: &Path, rev: &str) -> Result<CommitDetail> {
    let repo = open_repo(path)?;

    let obj = repo
        .revparse_single(rev)
        .with_context(|| format!("cannot resolve revision: {rev}"))?;

    let commit = obj
        .peel_to_commit()
        .with_context(|| format!("{rev} does not point to a commit"))?;

    let hash = commit.id().to_string();
    let author = commit
        .author()
        .name()
        .unwrap_or("<unknown>")
        .to_string();
    let email = commit.author().email().unwrap_or("").to_string();

    let time = commit.time();
    let date = crate::log::format_epoch(time.seconds());

    let message = commit
        .message()
        .unwrap_or("")
        .to_string();

    let tree = commit.tree().context("commit has no tree")?;

    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };

    let mut opts = git2::DiffOptions::new();
    opts.context_lines(3);

    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))
        .context("failed to compute commit diff")?;

    let stats = diff.stats().context("failed to compute diff stats")?;
    let stat = DiffStat {
        files_changed: stats.files_changed(),
        insertions: stats.insertions(),
        deletions: stats.deletions(),
    };

    let mut diff_entries = Vec::new();
    for (i, delta) in diff.deltas().enumerate() {
        let old_path = delta.old_file().path().map(|p| p.display().to_string());
        let new_path = delta.new_file().path().map(|p| p.display().to_string());

        let patch = match git2::Patch::from_diff(&diff, i)? {
            Some(mut patch) => {
                let buf = patch.to_buf()?;
                String::from_utf8_lossy(&buf).to_string()
            }
            None => String::new(),
        };

        diff_entries.push(DiffEntry {
            old_path,
            new_path,
            patch,
        });
    }

    Ok(CommitDetail {
        hash,
        author,
        email,
        date,
        message,
        diff_entries,
        stat,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_repo_with_two_commits() -> (TempDir, git2::Repository) {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();

        // Commit 1
        let file = dir.path().join("hello.txt");
        fs::write(&file, "hello\n").unwrap();
        {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("hello.txt")).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "first commit", &tree, &[])
                .unwrap();
        }

        // Commit 2
        fs::write(&file, "hello\nworld\n").unwrap();
        {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("hello.txt")).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                "add world line",
                &tree,
                &[&head],
            )
            .unwrap();
        }

        (dir, repo)
    }

    #[test]
    fn test_show_head() {
        let (dir, _) = init_repo_with_two_commits();
        let detail = show(dir.path(), "HEAD").unwrap();
        assert_eq!(detail.message, "add world line");
        assert_eq!(detail.stat.files_changed, 1);
        assert_eq!(detail.stat.insertions, 1);
        assert!(!detail.diff_entries.is_empty());
        assert!(detail.diff_entries[0].patch.contains("+world"));
    }

    #[test]
    fn test_show_first_commit() {
        let (dir, _) = init_repo_with_two_commits();
        let detail = show(dir.path(), "HEAD~1").unwrap();
        assert_eq!(detail.message, "first commit");
        assert!(detail.diff_entries[0].patch.contains("+hello"));
    }
}
