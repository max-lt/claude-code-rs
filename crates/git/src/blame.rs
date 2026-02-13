use std::path::Path;

use anyhow::{Context, Result};

use crate::repo::open_repo;

/// A single line from git blame output.
#[derive(Debug, Clone)]
pub struct BlameLine {
    pub line_number: usize,
    pub commit_hash: String,
    pub short_hash: String,
    pub author: String,
    pub date: String,
    pub content: String,
}

/// Blame a file — show who last modified each line (like `git blame`).
///
/// `file_path` is relative to the repo root.
pub fn blame(repo_path: &Path, file_path: &str) -> Result<Vec<BlameLine>> {
    let repo = open_repo(repo_path)?;

    let spec = repo
        .head()
        .context("cannot read HEAD")?
        .target()
        .context("HEAD has no target")?;

    let mut opts = git2::BlameOptions::new();
    opts.newest_commit(spec);

    let blame = repo
        .blame_file(Path::new(file_path), Some(&mut opts))
        .with_context(|| format!("failed to blame {file_path}"))?;

    // Read file content for line text
    let workdir = repo.workdir().context("bare repository")?;
    let full_path = workdir.join(file_path);
    let content = std::fs::read_to_string(&full_path)
        .with_context(|| format!("cannot read {}", full_path.display()))?;

    let lines: Vec<&str> = content.lines().collect();

    let mut result = Vec::with_capacity(lines.len());

    for (i, line_text) in lines.iter().enumerate() {
        let line_no = i + 1; // 1-based

        if let Some(hunk) = blame.get_line(line_no) {
            let oid = hunk.final_commit_id();
            let hash = oid.to_string();
            let short_hash = hash[..7.min(hash.len())].to_string();

            let sig = hunk.final_signature();
            let author = sig.name().unwrap_or("<unknown>").to_string();

            let date = if let Ok(commit) = repo.find_commit(oid) {
                crate::log::format_epoch(commit.time().seconds())
            } else {
                String::new()
            };

            result.push(BlameLine {
                line_number: line_no,
                commit_hash: hash,
                short_hash,
                author,
                date,
                content: line_text.to_string(),
            });
        }
    }

    Ok(result)
}

/// Blame a specific range of lines.
pub fn blame_range(
    repo_path: &Path,
    file_path: &str,
    start_line: usize,
    end_line: usize,
) -> Result<Vec<BlameLine>> {
    let all = blame(repo_path, file_path)?;

    Ok(all
        .into_iter()
        .filter(|l| l.line_number >= start_line && l.line_number <= end_line)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_repo_with_blame() -> (TempDir, git2::Repository) {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let sig = git2::Signature::now("Alice", "alice@test.com").unwrap();

        let file = dir.path().join("code.rs");
        fs::write(&file, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

        {
            let mut index = repo.index().unwrap();
            index.add_path(Path::new("code.rs")).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "initial code", &tree, &[])
                .unwrap();
        }

        (dir, repo)
    }

    #[test]
    fn test_blame_basic() {
        let (dir, _) = init_repo_with_blame();
        let lines = blame(dir.path(), "code.rs").unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].author, "Alice");
        assert_eq!(lines[0].line_number, 1);
        assert_eq!(lines[0].content, "fn main() {");
        assert_eq!(lines[2].content, "}");
    }

    #[test]
    fn test_blame_range() {
        let (dir, _) = init_repo_with_blame();
        let lines = blame_range(dir.path(), "code.rs", 2, 2).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].content.contains("println"));
    }
}
