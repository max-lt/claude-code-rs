use std::path::Path;

use super::{ToolDef, ToolOutput};

pub struct FileWriteTool;

impl ToolDef for FileWriteTool {
    fn name(&self) -> &'static str {
        "file_write"
    }

    fn description(&self) -> &'static str {
        "Write content to a file at the given path. Creates the file if it doesn't exist, \
         or overwrites it if it does."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to write to (absolute or relative to working directory)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, input: &serde_json::Value, cwd: &Path) -> ToolOutput {
        let path = match input.get("path").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: path"),
        };

        let content = match input.get("content").and_then(|c| c.as_str()) {
            Some(c) => c,
            None => return ToolOutput::error("Missing required parameter: content"),
        };

        let resolved = if Path::new(path).is_absolute() {
            Path::new(path).to_path_buf()
        } else {
            cwd.join(path)
        };

        // Ensure parent directories exist
        if let Some(parent) = resolved.parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            return ToolOutput::error(format!(
                "Failed to create directories for {}: {e}",
                resolved.display()
            ));
        }

        match tokio::fs::write(&resolved, content).await {
            Ok(()) => ToolOutput::success(format!(
                "Wrote {} bytes to {}",
                content.len(),
                resolved.display()
            )),
            Err(e) => ToolOutput::error(format!("Failed to write {}: {e}", resolved.display())),
        }
    }
}
