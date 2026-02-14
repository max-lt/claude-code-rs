use std::fmt::Write;
use std::path::Path;

use super::{ToolDef, ToolOutput};

pub struct ListTool;

impl ToolDef for ListTool {
    fn name(&self) -> &'static str {
        "List"
    }

    fn description(&self) -> &'static str {
        "List directory contents. Returns file names with type indicators (/ for directories, \
         @ for symlinks). Use this instead of `ls` via Bash."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory to list (defaults to working directory)"
                }
            }
        })
    }

    async fn execute(&self, input: &serde_json::Value, cwd: &Path) -> ToolOutput {
        let dir = match input.get("path").and_then(|p| p.as_str()) {
            Some(p) if Path::new(p).is_absolute() => Path::new(p).to_path_buf(),
            Some(p) => cwd.join(p),
            None => cwd.to_path_buf(),
        };

        if !dir.is_dir() {
            return ToolOutput::error(format!("Not a directory: {}", dir.display()));
        }

        let mut read_dir = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(e) => return ToolOutput::error(format!("Failed to read directory: {e}")),
        };

        let mut entries: Vec<(String, &str)> = Vec::new();

        loop {
            let entry = match read_dir.next_entry().await {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(e) => return ToolOutput::error(format!("Failed to read entry: {e}")),
            };

            let name = entry.file_name().to_string_lossy().into_owned();

            // Skip hidden files
            if name.starts_with('.') {
                continue;
            }

            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            let suffix = if file_type.is_dir() {
                "/"
            } else if file_type.is_symlink() {
                "@"
            } else {
                ""
            };

            entries.push((name, suffix));
        }

        if entries.is_empty() {
            return ToolOutput::success("(empty directory)");
        }

        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let mut out = String::new();

        for (name, suffix) in &entries {
            writeln!(out, "{name}{suffix}").unwrap();
        }

        // Remove trailing newline
        out.pop();

        ToolOutput::success(out)
    }
}
