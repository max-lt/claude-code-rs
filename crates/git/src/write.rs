//! Write operations: add, commit, push, reset, etc.

use anyhow::{Context, Result, bail};
use git2::{IndexAddOption, PushOptions, RemoteCallbacks, Signature};
use std::path::Path;

use crate::repo::open_repo;

/// Stage files matching a pattern (like `git add <pathspec>`)
pub fn add(cwd: &Path, pathspec: &[&str]) -> Result<()> {
    let repo = open_repo(cwd)?;
    let mut index = repo.index()?;

    index
        .add_all(pathspec, IndexAddOption::DEFAULT, None)
        .context("Failed to add files to index")?;

    index.write().context("Failed to write index")?;
    Ok(())
}

/// Unstage files (like `git reset <pathspec>`)
pub fn unstage(cwd: &Path, pathspec: &[&str]) -> Result<()> {
    let repo = open_repo(cwd)?;
    let head = repo.head()?.peel_to_commit()?;
    let obj = head.as_object();

    for path in pathspec {
        repo.reset_default(Some(obj), [path])?;
    }

    Ok(())
}

/// Create a commit with the staged changes
pub fn commit(cwd: &Path, message: &str) -> Result<String> {
    let repo = open_repo(cwd)?;

    // Get the signature (author/committer)
    let sig = repo
        .signature()
        .or_else(|_| Signature::now("Claude Code", "claude@anthropic.com"))?;

    // Get the tree from the index
    let mut index = repo.index()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;

    // Get parent commit (HEAD)
    let parent_commit = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(_) => None, // Initial commit
    };

    let parents = match &parent_commit {
        Some(p) => vec![p],
        None => vec![],
    };

    // Create the commit
    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;

    Ok(oid.to_string())
}

/// Push to remote
pub fn push(cwd: &Path, remote: &str, refspec: &str, force: bool) -> Result<String> {
    let repo = open_repo(cwd)?;
    let mut remote = repo
        .find_remote(remote)
        .context(format!("Remote '{}' not found", remote))?;

    let callbacks = RemoteCallbacks::new();

    // For now, we rely on ssh-agent or credential helper
    // Could add credential callback here if needed

    let mut push_opts = PushOptions::new();
    push_opts.remote_callbacks(callbacks);

    let refspecs = if force {
        vec![format!("+{}", refspec)]
    } else {
        vec![refspec.to_string()]
    };

    remote
        .push(&refspecs, Some(&mut push_opts))
        .context("Push failed")?;

    Ok(format!(
        "Pushed {} to {}",
        refspec,
        remote.name().unwrap_or("unknown")
    ))
}

/// Reset to a specific commit (soft, mixed, or hard)
pub fn reset(cwd: &Path, target: &str, mode: ResetMode) -> Result<()> {
    let repo = open_repo(cwd)?;

    let obj = repo
        .revparse_single(target)
        .context(format!("Failed to parse revision '{}'", target))?;

    let reset_type = match mode {
        ResetMode::Soft => git2::ResetType::Soft,
        ResetMode::Mixed => git2::ResetType::Mixed,
        ResetMode::Hard => git2::ResetType::Hard,
    };

    repo.reset(&obj, reset_type, None).context("Reset failed")?;

    Ok(())
}

/// Create a new branch
pub fn create_branch(cwd: &Path, name: &str, start_point: Option<&str>) -> Result<()> {
    let repo = open_repo(cwd)?;

    let commit = match start_point {
        Some(sp) => repo.revparse_single(sp)?.peel_to_commit()?,
        None => repo.head()?.peel_to_commit()?,
    };

    repo.branch(name, &commit, false)
        .context(format!("Failed to create branch '{}'", name))?;

    Ok(())
}

/// Switch to a branch (checkout)
pub fn checkout(cwd: &Path, branch_name: &str) -> Result<()> {
    let repo = open_repo(cwd)?;

    let (obj, reference) = repo
        .revparse_ext(branch_name)
        .context(format!("Failed to find branch '{}'", branch_name))?;

    repo.checkout_tree(&obj, None).context("Checkout failed")?;

    match reference {
        Some(r) => {
            repo.set_head(r.name().unwrap())?;
        }
        None => {
            repo.set_head_detached(obj.id())?;
        }
    }

    Ok(())
}

/// Delete a branch
pub fn delete_branch(cwd: &Path, name: &str, force: bool) -> Result<()> {
    let repo = open_repo(cwd)?;

    let mut branch = repo
        .find_branch(name, git2::BranchType::Local)
        .context(format!("Branch '{}' not found", name))?;

    if !force && !branch.is_head() {
        // Check if branch is merged
        let head = repo.head()?.peel_to_commit()?;
        let branch_commit = branch.get().peel_to_commit()?;

        if !repo.graph_descendant_of(head.id(), branch_commit.id())? {
            bail!(
                "Branch '{}' is not fully merged. Use force to delete anyway.",
                name
            );
        }
    }

    branch
        .delete()
        .context(format!("Failed to delete branch '{}'", name))?;

    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

impl std::str::FromStr for ResetMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "soft" => Ok(Self::Soft),
            "mixed" => Ok(Self::Mixed),
            "hard" => Ok(Self::Hard),
            other => Err(format!("Invalid reset mode: {other}")),
        }
    }
}
