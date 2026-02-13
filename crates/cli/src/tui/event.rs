use std::sync::mpsc as std_mpsc;

use tokio::sync::mpsc;

use claude_code_core::api::Usage;
use claude_code_core::event::EventHandler;

/// Events sent from the session task to the UI.
pub enum UiEvent {
    Text(String),
    Error(String),
    ToolStart {
        name: String,
    },
    ToolExecuting {
        input: serde_json::Value,
    },
    ToolResult {
        output: String,
        is_error: bool,
    },
    ToolEnd,
    Done(Usage),
    Failed(String),
    PermissionRequest {
        description: String,
        respond: std_mpsc::SyncSender<bool>,
    },
}

/// Commands sent from the UI to the session task.
pub enum SessionCmd {
    SendMessage(String),
    SetModel(String),
    Clear,
}

/// Bridges `EventHandler` trait calls into `UiEvent` channel sends.
pub struct ChannelEventHandler {
    pub tx: mpsc::UnboundedSender<UiEvent>,
}

impl EventHandler for ChannelEventHandler {
    fn on_text(&mut self, text: &str) {
        let _ = self.tx.send(UiEvent::Text(text.to_string()));
    }

    fn on_error(&mut self, message: &str) {
        let _ = self.tx.send(UiEvent::Error(message.to_string()));
    }

    fn on_tool_use_start(&mut self, name: &str, _id: &str) {
        let _ = self.tx.send(UiEvent::ToolStart {
            name: name.to_string(),
        });
    }

    fn on_tool_executing(&mut self, _name: &str, input: &serde_json::Value) {
        let _ = self.tx.send(UiEvent::ToolExecuting {
            input: input.clone(),
        });
    }

    fn on_tool_result(&mut self, _name: &str, output: &str, is_error: bool) {
        let _ = self.tx.send(UiEvent::ToolResult {
            output: output.to_string(),
            is_error,
        });
    }

    fn on_tool_use_end(&mut self, _name: &str) {
        let _ = self.tx.send(UiEvent::ToolEnd);
    }
}
