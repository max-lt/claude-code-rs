use colored::Colorize;

use super::CommandResult;

pub fn run() -> CommandResult {
    println!("{}", "Conversation cleared.".dimmed());
    CommandResult::Continue
}
