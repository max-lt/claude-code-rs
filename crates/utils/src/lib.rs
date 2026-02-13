/// Directories ignored by all file-walking tools (Glob, Grep, Search).
pub const IGNORED_DIRS: &[&str] = &[
    ".DS_Store",
    ".git",
    ".gradle",
    ".idea",
    ".next",
    ".nuxt",
    ".output",
    ".pytest_cache",
    ".svelte-kit",
    ".venv",
    ".vscode",
    "__pycache__",
    "build",
    "dist",
    "node_modules",
    "target",
    "venv",
];

/// Returns `true` if the directory name should be skipped during file walking.
pub fn is_ignored_dir(name: &str) -> bool {
    IGNORED_DIRS.contains(&name)
}
