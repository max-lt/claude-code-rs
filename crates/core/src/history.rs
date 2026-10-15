//! On-disk storage for conversations, so they can be resumed later.
//!
//! One JSON file per session under `<config dir>/sessions/<id>.json`. The
//! bootstrap messages (tool instructions) are not stored: they are rebuilt for
//! the current working directory when a session is restored.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::api::Message;
use crate::config;

const PREVIEW_CHARS: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub id: String,
    pub cwd: PathBuf,
    pub model: String,
    /// Seconds since the Unix epoch.
    pub updated_at: u64,
    pub messages: Vec<Message>,
    /// Conversation size after the last turn, in tokens. Absent in sessions
    /// saved before it was recorded; the session estimates it on restore.
    #[serde(default)]
    pub context_used: Option<u64>,
}

/// What the resume picker shows for one saved session.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub updated_at: u64,
    pub message_count: usize,
    /// Start of the first user message.
    pub preview: String,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A fresh, sortable session id (milliseconds since the Unix epoch).
pub fn new_id() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .to_string()
}

pub fn sessions_dir() -> Result<PathBuf> {
    let dir = config::config_dir()?.join("sessions");
    fs::create_dir_all(&dir).context("Failed to create sessions directory")?;
    Ok(dir)
}

pub fn save(session: &StoredSession) -> Result<()> {
    save_in(&sessions_dir()?, session)
}

pub fn load(id: &str) -> Result<StoredSession> {
    load_from(&sessions_dir()?, id)
}

/// Saved sessions started from `cwd`, most recent first.
pub fn list(cwd: &Path) -> Result<Vec<SessionSummary>> {
    list_in(&sessions_dir()?, cwd)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn session_path(dir: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        bail!("Invalid session id: {id:?}");
    }

    Ok(dir.join(format!("{id}.json")))
}

fn save_in(dir: &Path, session: &StoredSession) -> Result<()> {
    let path = session_path(dir, &session.id)?;
    let tmp = path.with_extension("json.tmp");
    let contents = serde_json::to_string(session)?;

    // Write then rename, so a crash mid-write never corrupts a saved session.
    fs::write(&tmp, contents).context("Failed to write session file")?;
    fs::rename(&tmp, &path).context("Failed to replace session file")?;

    Ok(())
}

fn load_from(dir: &Path, id: &str) -> Result<StoredSession> {
    let path = session_path(dir, id)?;

    if !path.exists() {
        bail!("No saved session with id {id}");
    }

    let contents = fs::read_to_string(&path).context("Failed to read session file")?;
    serde_json::from_str(&contents).with_context(|| format!("Session {id} is corrupted"))
}

fn list_in(dir: &Path, cwd: &Path) -> Result<Vec<SessionSummary>> {
    let mut sessions = Vec::new();

    for entry in fs::read_dir(dir).context("Failed to read sessions directory")? {
        let path = entry?.path();

        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }

        // Unreadable or foreign files are skipped, not fatal.
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(stored) = serde_json::from_str::<StoredSession>(&contents) else {
            continue;
        };

        if stored.cwd != cwd {
            continue;
        }

        sessions.push(summarize(&stored));
    }

    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(b.id.cmp(&a.id)));

    Ok(sessions)
}

fn summarize(stored: &StoredSession) -> SessionSummary {
    let first_user = stored
        .messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.to_text())
        .find(|t| !t.trim().is_empty())
        .unwrap_or_default();

    let flat = first_user.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview = if flat.chars().count() > PREVIEW_CHARS {
        let cut: String = flat.chars().take(PREVIEW_CHARS).collect();
        format!("{cut}…")
    } else {
        flat
    };

    SessionSummary {
        id: stored.id.clone(),
        updated_at: stored.updated_at,
        message_count: stored.messages.len(),
        preview,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Content;

    fn stored(id: &str, cwd: &str, updated_at: u64, first: &str) -> StoredSession {
        StoredSession {
            id: id.to_string(),
            cwd: PathBuf::from(cwd),
            model: "claude-sonnet-5-5".to_string(),
            updated_at,
            context_used: None,
            messages: vec![
                Message {
                    role: "user".to_string(),
                    content: Content::text(first),
                },
                Message {
                    role: "assistant".to_string(),
                    content: Content::text("ok"),
                },
            ],
        }
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        save_in(tmp.path(), &stored("1", "/p", 10, "hello")).unwrap();

        let loaded = load_from(tmp.path(), "1").unwrap();

        assert_eq!(loaded.id, "1");
        assert_eq!(loaded.cwd, PathBuf::from("/p"));
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].content.to_text(), "hello");
    }

    #[test]
    fn context_used_roundtrips_and_old_files_still_load() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = stored("1", "/p", 10, "hello");
        s.context_used = Some(1234);
        save_in(tmp.path(), &s).unwrap();
        assert_eq!(load_from(tmp.path(), "1").unwrap().context_used, Some(1234));

        // A file written before the field existed.
        let mut json: serde_json::Value = serde_json::to_value(stored("2", "/p", 10, "hi")).unwrap();
        json.as_object_mut().unwrap().remove("context_used");
        fs::write(tmp.path().join("2.json"), json.to_string()).unwrap();
        assert_eq!(load_from(tmp.path(), "2").unwrap().context_used, None);
    }

    #[test]
    fn load_missing_session_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load_from(tmp.path(), "nope").unwrap_err();

        assert!(err.to_string().contains("No saved session"));
    }

    #[test]
    fn ids_cannot_escape_the_directory() {
        let tmp = tempfile::tempdir().unwrap();

        assert!(load_from(tmp.path(), "../credentials").is_err());
        assert!(load_from(tmp.path(), "").is_err());
    }

    #[test]
    fn list_filters_by_cwd_and_sorts_recent_first() {
        let tmp = tempfile::tempdir().unwrap();
        save_in(tmp.path(), &stored("a", "/p", 10, "old")).unwrap();
        save_in(tmp.path(), &stored("b", "/p", 30, "new")).unwrap();
        save_in(tmp.path(), &stored("c", "/other", 20, "elsewhere")).unwrap();

        let found = list_in(tmp.path(), Path::new("/p")).unwrap();
        let ids: Vec<_> = found.iter().map(|s| s.id.as_str()).collect();

        assert_eq!(ids, vec!["b", "a"]);
    }

    #[test]
    fn list_skips_garbage_files() {
        let tmp = tempfile::tempdir().unwrap();
        save_in(tmp.path(), &stored("a", "/p", 10, "hi")).unwrap();
        fs::write(tmp.path().join("broken.json"), "not json").unwrap();
        fs::write(tmp.path().join("notes.txt"), "hello").unwrap();

        let found = list_in(tmp.path(), Path::new("/p")).unwrap();

        assert_eq!(found.len(), 1);
    }

    #[test]
    fn preview_is_flattened_and_truncated() {
        let long = format!("line one\n{}", "x".repeat(200));
        let summary = summarize(&stored("a", "/p", 1, &long));

        assert!(summary.preview.starts_with("line one xxx"));
        assert!(summary.preview.ends_with('…'));
        assert_eq!(summary.preview.chars().count(), PREVIEW_CHARS + 1);
        assert_eq!(summary.message_count, 2);
    }
}
