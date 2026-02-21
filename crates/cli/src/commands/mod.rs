mod clear;
mod help;
mod model;
mod quit;
mod think;
#[cfg(feature = "voice")]
pub mod rec;

use claude_code_core::api::ThinkingConfig;

#[allow(dead_code)]
pub enum CommandResult {
    Continue,
    Exit,
    Clear,
    SetModel {
        id: String,
        label: String,
    },
    SetThinking(ThinkingConfig),
    Info(String),
    #[cfg(feature = "voice")]
    SendMessage(String),
    #[cfg(feature = "voice")]
    RecordVoice,
}

/// Try to handle input as a slash command.
/// Returns `None` if the input is not a command.
pub fn handle_command(input: &str, current_model: &str) -> Option<CommandResult> {
    let cmd = input.split_whitespace().next()?;

    match cmd {
        "/help" | "/h" => Some(help::run()),
        "/quit" | "/exit" | "/q" => Some(quit::run()),
        "/clear" => Some(clear::run()),
        "/model" => {
            let args = input.strip_prefix("/model").unwrap_or("").trim();
            Some(model::run(args, current_model))
        }
        "/think" => {
            let args = input.strip_prefix("/think").unwrap_or("").trim();
            Some(think::run(args))
        }
        #[cfg(feature = "voice")]
        "/rec" => Some(CommandResult::RecordVoice),
        _ if cmd.starts_with('/') => Some(CommandResult::Info(format!(
            "Unknown command: {cmd}. Type /help for available commands."
        ))),
        _ => None,
    }
}
