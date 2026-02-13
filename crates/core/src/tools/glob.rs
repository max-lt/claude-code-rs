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

        let full_pattern = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            format!("{}/{pattern}", base_dir.display())
        };

        let entries = match glob::glob(&full_pattern) {
            Ok(paths) => paths,
            Err(e) => return ToolOutput::error(format!("Invalid glob pattern: {e}")),
        };

        let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();

        for entry in entries {
            let path = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };

            if path.is_file() {
                let mtime = path
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                files.push((path, mtime));
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
