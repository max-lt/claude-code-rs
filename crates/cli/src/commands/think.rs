use claude_code_core::api::ThinkingConfig;

use super::CommandResult;

pub fn run(args: &str) -> CommandResult {
    match args {
        "" => CommandResult::Info(
            "Usage:\n  \
             /think off      — Disable extended thinking\n  \
             /think adaptive — Adaptive thinking (model decides, best for Opus 4.6)\n  \
             /think <budget> — Enable thinking with token budget (e.g. /think 10000)"
                .to_string(),
        ),
        "off" | "disable" | "none" => {
            CommandResult::SetThinking(ThinkingConfig::Disabled)
        }
        "adaptive" | "auto" => {
            CommandResult::SetThinking(ThinkingConfig::Adaptive)
        }
        budget => match budget.parse::<u32>() {
            Ok(n) if n >= 1024 => {
                CommandResult::SetThinking(ThinkingConfig::Enabled { budget_tokens: n })
            }
            Ok(_) => CommandResult::Info(
                "Budget must be at least 1024 tokens.".to_string(),
            ),
            Err(_) => CommandResult::Info(format!(
                "Invalid argument: \"{budget}\". Use 'off', 'adaptive', or a number >= 1024."
            )),
        },
    }
}
