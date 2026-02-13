use std::path::Path;

use anyhow::{Context, Result};
use git2::Sort;

use crate::repo::open_repo;

/// A single commit entry.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub message: String,
}

/// Return the last `limit` commits from HEAD (like `git log --oneline -n`).
pub fn log(path: &Path, limit: usize) -> Result<Vec<LogEntry>> {
    let repo = open_repo(path)?;

    let head = match repo.head() {
        Ok(h) => h,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };

    let head_oid = head.target().context("HEAD has no target")?;

    let mut revwalk = repo.revwalk()?;
    revwalk.set_sorting(Sort::TIME | Sort::TOPOLOGICAL)?;
    revwalk.push(head_oid)?;

    let mut entries = Vec::with_capacity(limit);

    for oid in revwalk.take(limit) {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;

        let hash = oid.to_string();
        let short_hash = hash[..7.min(hash.len())].to_string();

        let author = commit.author().name().unwrap_or("<unknown>").to_string();
        let email = commit.author().email().unwrap_or("").to_string();

        let time = commit.time();
        let date = format_epoch(time.seconds());

        let message = commit
            .message()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("")
            .to_string();

        entries.push(LogEntry {
            hash,
            short_hash,
            author,
            email,
            date,
            message,
        });
    }

    Ok(entries)
}

/// Basic epoch → "YYYY-MM-DD HH:MM" formatter (UTC, no chrono dependency).
pub fn format_epoch(epoch: i64) -> String {
    // We avoid pulling chrono just for this. Rough UTC conversion.
    let secs_per_min = 60;
    let secs_per_hour = 3600;
    let secs_per_day = 86400;

    let days = epoch / secs_per_day;
    let remainder = epoch % secs_per_day;
    let hour = remainder / secs_per_hour;
    let minute = (remainder % secs_per_hour) / secs_per_min;

    // Days since 1970-01-01
    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Convert days since epoch to (year, month, day) — civil calendar, UTC.
fn days_to_ymd(mut days: i64) -> (i64, i64, i64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_repo_with_commits(n: usize) -> (TempDir, git2::Repository) {
        let dir = TempDir::new().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let sig = git2::Signature::now("Test", "test@test.com").unwrap();

        let file = dir.path().join("file.txt");

        for i in 0..n {
            fs::write(&file, format!("commit {i}")).unwrap();

            let mut index = repo.index().unwrap();
            index.add_path(Path::new("file.txt")).unwrap();
            index.write().unwrap();

            let tree_id = index.write_tree().unwrap();
            let tree = repo.find_tree(tree_id).unwrap();

            let parents: Vec<git2::Commit> = if i == 0 {
                vec![]
            } else {
                let head = repo.head().unwrap().peel_to_commit().unwrap();
                vec![head]
            };

            let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
            repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &format!("commit message {i}"),
                &tree,
                &parent_refs,
            )
            .unwrap();
        }

        (dir, repo)
    }

    #[test]
    fn test_log_limit() {
        let (dir, _) = init_repo_with_commits(5);
        let entries = log(dir.path(), 3).unwrap();
        assert_eq!(entries.len(), 3);
        // Most recent first (topological order)
        assert!(entries[0].message.contains("4"));
        assert!(entries[1].message.contains("3"));
    }

    #[test]
    fn test_log_all() {
        let (dir, _) = init_repo_with_commits(3);
        let entries = log(dir.path(), 100).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_format_epoch() {
        // 2024-01-15 12:30 UTC = 1705321800
        let s = format_epoch(1705321800);
        assert!(s.starts_with("2024-01-15"));
    }
}
