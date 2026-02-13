use std::path::Path;

use tokio::process::Command;

use super::{ToolDef, ToolOutput};

pub struct BashTool;

impl ToolDef for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Execute a bash command and return its output."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: &serde_json::Value, cwd: &Path) -> ToolOutput {
        let command = match input.get("command").and_then(|c| c.as_str()) {
            Some(c) => c,
            None => return ToolOutput::error("Missing required parameter: command"),
        };

        let result = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .output()
            .await;

        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let mut content = String::new();

                if !stdout.is_empty() {
                    content.push_str(&stdout);
                }

                if !stderr.is_empty() {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str("stderr:\n");
                    content.push_str(&stderr);
                }

                if content.is_empty() {
                    content.push_str("(no output)");
                }

                if output.status.success() {
                    ToolOutput::success(content)
                } else {
                    let code = output.status.code().unwrap_or(-1);
                    ToolOutput::error(format!("Exit code {code}\n{content}"))
                }
            }
            Err(e) => ToolOutput::error(format!("Failed to execute command: {e}")),
        }
    }
}
