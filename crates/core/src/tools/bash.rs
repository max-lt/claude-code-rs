use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

use super::{ToolDef, ToolOutput};

pub struct BashTool;

impl ToolDef for BashTool {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn description(&self) -> &'static str {
        "Executes a bash command. Use for running programs, installing packages, git operations, \
         builds, and other terminal tasks. Do NOT use for reading or writing files — use the \
         Read and Write tools instead."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds (max 600000, default 120000)"
                },
                "description": {
                    "type": "string",
                    "description": "A short description of what this command does"
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

        let timeout_ms = input
            .get("timeout")
            .and_then(|t| t.as_u64())
            .unwrap_or(120_000)
            .min(600_000);

        let result = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            Command::new("bash")
                .arg("-c")
                .arg(command)
                .current_dir(cwd)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
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
            Ok(Err(e)) => ToolOutput::error(format!("Failed to execute command: {e}")),
            Err(_) => ToolOutput::error(format!("Command timed out after {timeout_ms}ms")),
        }
    }
}
