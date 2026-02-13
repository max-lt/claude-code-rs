mod clear;
mod help;
mod model;
mod quit;
#[cfg(feature = "voice")]
pub mod rec;

pub enum CommandResult {
    Continue,
    Exit,
    SetModel { id: String, label: String },
    #[cfg(feature = "voice")]
    SendMessage(String),
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
        _ if cmd.starts_with('/') => {
            eprintln!("Unknown command: {cmd}. Type /help for available commands.");
            Some(CommandResult::Continue)
        }
        _ => None,
    }
}
