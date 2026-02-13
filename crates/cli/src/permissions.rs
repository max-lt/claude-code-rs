use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;

use tokio::sync::mpsc;

use claude_code_core::permission::{PermissionConfig, PermissionHandler, Tool};

use crate::tui::UiEvent;

/// Channel-based permission handler for the TUI.
///
/// On rule miss, sends a `UiEvent::PermissionRequest` with a oneshot channel
/// and blocks the current thread waiting for the UI's y/n response.
pub struct ChannelPermissions {
    config: PermissionConfig,
    project_dir: PathBuf,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
}

impl ChannelPermissions {
    pub fn new(
        config: PermissionConfig,
        project_dir: PathBuf,
        ui_tx: mpsc::UnboundedSender<UiEvent>,
    ) -> Self {
        Self {
            config,
            project_dir,
            ui_tx,
        }
    }
}

impl PermissionHandler for ChannelPermissions {
    fn allow(&mut self, tool: &Tool<'_>) -> bool {
        // Check rule-based config first
        if let Some(allowed) = self.config.check(tool, &self.project_dir) {
            return allowed;
        }

        // No matching rule — ask the UI
        let description = match tool {
            Tool::Bash { command } => format!("Run command: {command}"),
            Tool::Read { path } => format!("Read file: {}", path.display()),
            Tool::Write { path } => format!("Write file: {}", path.display()),
            Tool::Edit { path } => format!("Edit file: {}", path.display()),
            Tool::Git => "Git repository operation".to_string(),
            Tool::Glob => "Search files by pattern".to_string(),
            Tool::Grep => "Search file contents".to_string(),
            Tool::Search => "Full-text search across codebase".to_string(),
            _ => "Unknown tool action".to_string(),
        };

        let (tx, rx) = std_mpsc::sync_channel(1);

        let _ = self.ui_tx.send(UiEvent::PermissionRequest {
            description,
            respond: tx,
        });

        // Block until the UI responds — safe because this runs in a spawned
        // tokio task, blocking only one worker thread.
        rx.recv().unwrap_or(false)
    }
}
