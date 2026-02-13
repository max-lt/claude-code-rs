//! File walking with mtime-based change tracking.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use ignore::WalkBuilder;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const TEXT_EXTENSIONS: &[&str] = &[
    // Programming
    "rs",
    "py",
    "js",
    "ts",
    "tsx",
    "jsx",
    "go",
    "java",
    "c",
    "cpp",
    "h",
    "hpp",
    "cs",
    "rb",
    "php",
    "swift",
    "kt",
    "scala",
    "clj",
    "ex",
    "exs",
    "erl",
    "hs",
    "ml",
    "fs",
    "r",
    "jl",
    "lua",
    "pl",
    "pm",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "cmd",
    // Web
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    "vue",
    "svelte",
    // Data
    "json",
    "yaml",
    "yml",
    "toml",
    "xml",
    "csv",
    "sql",
    // Docs
    "md",
    "txt",
    "rst",
    "org",
    "adoc",
    "tex",
    // Config
    "ini",
    "cfg",
    "conf",
    "env",
    "properties",
    "gradle",
    // Other
    "dockerfile",
    "makefile",
    "cmake",
    "lock",
    "gitignore",
    "editorconfig",
];

const MAX_FILE_SIZE: u64 = 1_048_576; // 1 MB

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub(crate) struct FileEntry {
    pub relative: String,
    pub content: String,
}

pub(crate) struct FileChange {
    pub relative: String,
    pub content: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeKind {
    Added,
    Modified,
}

pub(crate) struct IncrementalResult {
    pub changes: Vec<FileChange>,
    pub removed: Vec<String>,
}

pub struct WalkStats {
    pub files: usize,
    pub bytes: u64,
}

// ---------------------------------------------------------------------------
// FileWalker
// ---------------------------------------------------------------------------

pub(crate) struct FileWalker {
    root_dir: PathBuf,
    mtimes: HashMap<String, (u64, u32)>,
}

impl FileWalker {
    pub fn new(root_dir: PathBuf) -> Self {
        Self {
            root_dir,
            mtimes: HashMap::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root_dir
    }

    /// Walk all files, record mtimes, return entries.
    pub fn walk_all(&mut self) -> Result<(Vec<FileEntry>, WalkStats)> {
        let mut entries = Vec::new();
        let mut stats = WalkStats { files: 0, bytes: 0 };

        self.mtimes.clear();

        for entry in self.walker() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }

            let path = entry.path();

            if !is_text_file(path) {
                continue;
            }

            let metadata = match path.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            if metadata.len() > MAX_FILE_SIZE {
                continue;
            }

            let content = match std::fs::read(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if is_binary(&content) {
                continue;
            }

            let text = match String::from_utf8(content) {
                Ok(t) => t,
                Err(_) => continue,
            };

            let relative = path
                .strip_prefix(&self.root_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            if let Some(mtime) = get_mtime(path) {
                self.mtimes.insert(relative.clone(), mtime);
            }

            stats.files += 1;
            stats.bytes += metadata.len();

            entries.push(FileEntry {
                relative,
                content: text,
            });
        }

        Ok((entries, stats))
    }

    /// Walk incrementally: compare mtimes, return only changes.
    pub fn walk_incremental(&mut self) -> Result<IncrementalResult> {
        let mut changes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut new_mtimes = HashMap::new();

        for entry in self.walker() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }

            let path = entry.path();

            if !is_text_file(path) {
                continue;
            }

            let metadata = match path.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            if metadata.len() > MAX_FILE_SIZE {
                continue;
            }

            let relative = path
                .strip_prefix(&self.root_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            seen.insert(relative.clone());
            let current_mtime = get_mtime(path);

            // Check if unchanged
            if let Some(old_mtime) = self.mtimes.get(&relative)
                && current_mtime.as_ref() == Some(old_mtime)
            {
                new_mtimes.insert(relative, *old_mtime);
                continue;
            }

            // Added or modified — read content
            let content = match std::fs::read(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if is_binary(&content) {
                continue;
            }

            let text = match String::from_utf8(content) {
                Ok(t) => t,
                Err(_) => continue,
            };

            let kind = if self.mtimes.contains_key(&relative) {
                ChangeKind::Modified
            } else {
                ChangeKind::Added
            };

            if let Some(mtime) = current_mtime {
                new_mtimes.insert(relative.clone(), mtime);
            }

            changes.push(FileChange {
                relative,
                content: text,
                kind,
            });
        }

        // Detect removed files
        let removed: Vec<String> = self
            .mtimes
            .keys()
            .filter(|k| !seen.contains(k.as_str()))
            .cloned()
            .collect();

        // Carry forward unchanged mtimes
        for (k, v) in &self.mtimes {
            if seen.contains(k.as_str()) && !new_mtimes.contains_key(k) {
                new_mtimes.insert(k.clone(), *v);
            }
        }

        self.mtimes = new_mtimes;

        Ok(IncrementalResult { changes, removed })
    }

    fn walker(&self) -> ignore::Walk {
        WalkBuilder::new(&self.root_dir)
            .hidden(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
            .add_custom_ignore_filename(".claudeignore")
            // Add common build/dependency directories to ignore
            .filter_entry(|entry| {
                let name = entry
                    .path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                !ccrs_utils::is_ignored_dir(name)
            })
            .build()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn get_mtime(path: &Path) -> Option<(u64, u32)> {
    let meta = path.metadata().ok()?;
    let modified = meta.modified().ok()?;
    let duration = modified.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some((duration.as_secs(), duration.subsec_nanos()))
}

pub(crate) fn is_text_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    TEXT_EXTENSIONS.contains(&ext.as_str())
        || matches!(
            filename.as_str(),
            "dockerfile" | "makefile" | "rakefile" | "gemfile" | "procfile" | "readme"
        )
}

pub(crate) fn is_binary(buf: &[u8]) -> bool {
    buf.iter().take(8192).any(|&b| b == 0)
}
