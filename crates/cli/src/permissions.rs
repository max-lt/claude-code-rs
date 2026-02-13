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
                format!("Run command: {}", command.bold())
            }
            Tool::Read { path } => {
                format!("Read file: {}", path.display().to_string().bold())
            }
            Tool::Write { path } => {
                format!("Write file: {}", path.display().to_string().bold())
            }
            Tool::Edit { path } => {
                format!("Edit file: {}", path.display().to_string().bold())
            }
            Tool::Glob => "Search files by pattern".to_string(),
            Tool::Grep => "Search file contents".to_string(),
            _ => "Unknown tool action".to_string(),
        };

        println!("\n{} {description}", "Permission:".yellow().bold());

        Confirm::new()
            .with_prompt("Allow?")
            .default(true)
            .interact()
            .unwrap_or(false)
    }
}
