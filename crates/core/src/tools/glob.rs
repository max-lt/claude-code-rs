use std::path::Path;

use super::{ToolDef, ToolOutput};

pub struct GlobTool;

impl ToolDef for GlobTool {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn description(&self) -> &'static str {
        "Fast file pattern matching tool. Supports glob patterns like \"**/*.rs\" or \"src/**/*.ts\". \
         Returns matching file paths sorted by modification time."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match files against"
                },
                "path": {
                    "type": "string",
                    "description": "The directory to search in (defaults to working directory)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, input: &serde_json::Value, cwd: &Path) -> ToolOutput {
        let pattern = match input.get("pattern").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: pattern"),
        };

        let base_dir = match input.get("path").and_then(|p| p.as_str()) {
            Some(p) if Path::new(p).is_absolute() => Path::new(p).to_path_buf(),
            Some(p) => cwd.join(p),
            None => cwd.to_path_buf(),
        };

        // Compile glob pattern
        let glob_pattern = match glob::Pattern::new(pattern) {
            Ok(p) => p,
            Err(e) => return ToolOutput::error(format!("Invalid glob pattern: {e}")),
        };

        let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();

        // Use ignore::WalkBuilder with the same filters as search
        let walker = ignore::WalkBuilder::new(&base_dir)
            .hidden(false)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(false)
            .add_custom_ignore_filename(".claudeignore")
            .filter_entry(|entry| {
                let name = entry
                    .path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                !ccrs_utils::is_ignored_dir(name)
            })
            .build();

        for result in walker {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            // Match against glob pattern
            // Use path relative to base_dir for matching
            let rel_path = match path.strip_prefix(&base_dir) {
                Ok(p) => p,
                Err(_) => path,
            };

            if glob_pattern.matches_path(rel_path) {
                let mtime = path
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                files.push((path.to_path_buf(), mtime));
            }
        }

        // Sort by modification time, most recent first
        files.sort_by(|a, b| b.1.cmp(&a.1));

        if files.is_empty() {
            return ToolOutput::success("No files matched the pattern.");
        }

        let result: Vec<String> = files.iter().map(|(p, _)| p.display().to_string()).collect();
        ToolOutput::success(result.join("\n"))
    }
}
