use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use git2::Repository;

/// Open the git repository that contains `path`.
pub fn open_repo(path: &Path) -> Result<Repository> {
    Repository::discover(path).with_context(|| format!("no git repository at {}", path.display()))
}

/// Return the working directory root of the repository containing `path`.
pub fn repo_root(path: &Path) -> Result<PathBuf> {
    let repo = open_repo(path)?;
    repo.workdir()
        .map(|p| p.to_path_buf())
        .context("bare repository has no working directory")
}

/// Return the name of the current branch (HEAD), or `None` if detached.
pub fn current_branch(path: &Path) -> Result<Option<String>> {
    let repo = open_repo(path)?;
    let head = repo.head().context("failed to read HEAD")?;
    Ok(head.shorthand().map(|s| s.to_string()))
}

/// Information about a branch.
pub struct BranchInfo {
    pub name: String,
    pub is_head: bool,
    pub is_remote: bool,
}

/// List all local (and optionally remote) branches.
pub fn list_branches(path: &Path, include_remote: bool) -> Result<Vec<BranchInfo>> {
    let repo = open_repo(path)?;
    let filter = if include_remote {
        git2::BranchType::Remote
    } else {
        git2::BranchType::Local
    };

    let mut branches = Vec::new();

    // Local branches
    for entry in repo.branches(Some(git2::BranchType::Local))? {
        let (branch, _) = entry?;
        if let Some(name) = branch.name()? {
            branches.push(BranchInfo {
                name: name.to_string(),
                is_head: branch.is_head(),
                is_remote: false,
            });
        }
    }

    // Remote branches
    if include_remote {
        for entry in repo.branches(Some(filter))? {
            let (branch, _) = entry?;
            if let Some(name) = branch.name()? {
                branches.push(BranchInfo {
                    name: name.to_string(),
                    is_head: false,
                    is_remote: true,
                });
            }
        }
    }

    Ok(branches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn init_repo() -> (TempDir, Repository) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        // Need an initial commit for HEAD to exist
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
    fn test_open_repo() {
        let (dir, _) = init_repo();
        assert!(open_repo(dir.path()).is_ok());
    }

    #[test]
    fn test_repo_root() {
        let (dir, _) = init_repo();
        let root = repo_root(dir.path()).unwrap();
        assert_eq!(
            root.canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn test_current_branch() {
        let (dir, _) = init_repo();
        let branch = current_branch(dir.path()).unwrap();
        // Default branch after init with a commit
        assert!(branch.is_some());
    }

    #[test]
    fn test_list_branches() {
        let (dir, _) = init_repo();
        let branches = list_branches(dir.path(), false).unwrap();
        assert!(!branches.is_empty());
        assert!(branches.iter().any(|b| b.is_head));
    }

    #[test]
    fn test_no_repo() {
        let dir = TempDir::new().unwrap();
        assert!(open_repo(dir.path()).is_err());
    }
}
