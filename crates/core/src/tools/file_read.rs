use std::path::Path;

use super::{ToolDef, ToolOutput};

pub struct FileReadTool;

impl ToolDef for FileReadTool {
    fn name(&self) -> &'static str {
        "file_read"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file at the given path."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to read (absolute or relative to working directory)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: &serde_json::Value, cwd: &Path) -> ToolOutput {
        let path = match input.get("path").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: path"),
        };

        let resolved = if Path::new(path).is_absolute() {
            Path::new(path).to_path_buf()
        } else {
            cwd.join(path)
        };

        match tokio::fs::read_to_string(&resolved).await {
            Ok(content) => ToolOutput::success(content),
            Err(e) => ToolOutput::error(format!("Failed to read {}: {e}", resolved.display())),
        }
    }
}
