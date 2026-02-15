mod clear;
mod help;
mod model;
mod quit;
#[cfg(feature = "voice")]
pub mod rec;

#[allow(dead_code)]
pub enum CommandResult {
    Continue,
    Exit,
    Clear,
    SetModel {
        id: String,
        label: String,
    },
    Info(String),
    #[cfg(feature = "voice")]
    SendMessage(String),
    #[cfg(feature = "voice")]
    RecordVoice,
}

/// A slash command definition for autocomplete suggestions.
pub struct CommandDef {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
}

/// Returns all available slash commands.
pub fn available_commands() -> Vec<CommandDef> {
    let mut cmds = vec![
        CommandDef {
            name: "/help",
            aliases: &["/h"],
            description: "Show this help message",
        },
        CommandDef {
            name: "/quit",
            aliases: &["/q", "/exit"],
            description: "Exit the application",
        },
        CommandDef {
            name: "/clear",
            aliases: &[],
            description: "Clear conversation history",
        },
        CommandDef {
            name: "/model",
            aliases: &[],
            description: "List or switch models",
        },
    ];

    #[cfg(feature = "voice")]
    cmds.push(CommandDef {
        name: "/rec",
        aliases: &[],
        description: "Record and transcribe voice input",
    });

    cmds
}

/// Returns commands matching the given prefix (filters by name and aliases).
pub fn matching_commands(prefix: &str) -> Vec<&'static CommandDef> {
    // Leak a static reference so we can return &'static refs.
    // This is fine since available_commands() is small and deterministic.
    static COMMANDS: std::sync::OnceLock<Vec<CommandDef>> = std::sync::OnceLock::new();
    let commands = COMMANDS.get_or_init(available_commands);

    let prefix_lower = prefix.to_lowercase();

    commands
        .iter()
        .filter(|cmd| {
            cmd.name.starts_with(&prefix_lower)
                || cmd.aliases.iter().any(|a| a.starts_with(&prefix_lower))
        })
        .collect()
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
        #[cfg(feature = "voice")]
        "/rec" => Some(CommandResult::RecordVoice),
        _ if cmd.starts_with('/') => Some(CommandResult::Info(format!(
            "Unknown command: {cmd}. Type /help for available commands."
        ))),
        _ => None,
    }
}
