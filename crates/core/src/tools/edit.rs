use std::path::Path;

use super::{ToolDef, ToolOutput};

pub struct EditTool;

impl ToolDef for EditTool {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn description(&self) -> &'static str {
        "Performs exact string replacements in files. The old_string must be unique in the file. \
         Use replace_all to change every occurrence of old_string."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to modify"
                },
                "old_string": {
                    "type": "string",
                    "description": "The text to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to replace it with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default false)"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, input: &serde_json::Value, cwd: &Path) -> ToolOutput {
        let file_path = match input.get("file_path").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: file_path"),
        };

        let old_string = match input.get("old_string").and_then(|s| s.as_str()) {
            Some(s) => s,
            None => return ToolOutput::error("Missing required parameter: old_string"),
        };

        let new_string = match input.get("new_string").and_then(|s| s.as_str()) {
            Some(s) => s,
            None => return ToolOutput::error("Missing required parameter: new_string"),
        };

        let replace_all = input
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let resolved = if Path::new(file_path).is_absolute() {
            Path::new(file_path).to_path_buf()
        } else {
            cwd.join(file_path)
        };

        let content = match tokio::fs::read_to_string(&resolved).await {
            Ok(c) => c,
            Err(e) => {
                return ToolOutput::error(format!("Failed to read {}: {e}", resolved.display()));
            }
        };

        if old_string == new_string {
            return ToolOutput::error("old_string and new_string must be different");
        }

        let count = content.matches(old_string).count();

        if count == 0 {
            return ToolOutput::error(format!("old_string not found in {}", resolved.display()));
        }

        if !replace_all && count > 1 {
            return ToolOutput::error(format!(
                "old_string is not unique in {} ({count} occurrences). \
                 Provide more context to make it unique, or use replace_all.",
                resolved.display()
            ));
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        match tokio::fs::write(&resolved, &new_content).await {
            Ok(()) => {
                let msg = if replace_all {
                    format!("Replaced {count} occurrences in {}", resolved.display())
                } else {
                    format!("Edited {}", resolved.display())
                };

                ToolOutput::success(msg)
            }
            Err(e) => ToolOutput::error(format!("Failed to write {}: {e}", resolved.display())),
        }
    }
}
