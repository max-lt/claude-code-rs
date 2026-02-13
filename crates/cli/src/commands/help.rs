use colored::Colorize;

use super::CommandResult;

pub fn run() -> CommandResult {
    println!();
    println!("{}", "Available commands:".bold());
    println!(
        "  {} {}  — Show this help message",
        "/help".cyan(),
        "/h".dimmed()
    );
    println!(
        "  {} {} — Exit the application",
        "/quit".cyan(),
        "/q /exit".dimmed()
    );
    println!("  {}       — Clear conversation history", "/clear".cyan());
    println!("  {}       — List or switch models", "/model".cyan());
    #[cfg(feature = "voice")]
    println!(
        "  {}         — Record and transcribe voice input",
        "/rec".cyan()
    );
    println!();

    CommandResult::Continue
}
