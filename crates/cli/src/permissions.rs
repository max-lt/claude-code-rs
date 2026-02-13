use std::path::PathBuf;

use colored::Colorize;
use dialoguer::Confirm;

use claude_code_core::permission::{PermissionConfig, PermissionHandler, Tool};

pub struct InteractivePermissions {
    config: PermissionConfig,
    project_dir: PathBuf,
}

impl InteractivePermissions {
    pub fn new(config: PermissionConfig, project_dir: PathBuf) -> Self {
        Self {
            config,
            project_dir,
        }
    }
}

impl PermissionHandler for InteractivePermissions {
    fn allow(&mut self, tool: &Tool<'_>) -> bool {
        // Check rule-based config first
        if let Some(allowed) = self.config.check(tool, &self.project_dir) {
            return allowed;
        }

        // No matching rule — prompt interactively
        let description = match tool {
            Tool::Bash { command } => {
                format!("Run bash command: {}", command.bold())
            }
            Tool::FileRead { path } => {
                format!("Read file: {}", path.display().to_string().bold())
            }
            Tool::FileWrite { path } => {
                format!("Write file: {}", path.display().to_string().bold())
            }
            _ => "Unknown tool action".to_string(),
        };

        println!("\n{} {description}", "Tool:".yellow().bold());

        Confirm::new()
            .with_prompt("Allow?")
            .default(true)
            .interact()
            .unwrap_or(false)
    }
}
