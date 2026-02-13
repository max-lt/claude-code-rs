use colored::Colorize;

use claude_code_core::api::{AVAILABLE_MODELS, DEFAULT_MODEL};

use super::CommandResult;

pub fn run(args: &str, current_model: &str) -> CommandResult {
    let requested = args.trim();

    if requested.is_empty() {
        list_models(current_model);
        return CommandResult::Continue;
    }

    // Try exact match first, then prefix match
    let matched = AVAILABLE_MODELS
        .iter()
        .find(|(id, _)| *id == requested)
        .or_else(|| {
            AVAILABLE_MODELS.iter().find(|(id, label)| {
                id.contains(requested) || label.to_lowercase().contains(&requested.to_lowercase())
            })
        });

    match matched {
        Some((id, label)) => CommandResult::SetModel {
            id: id.to_string(),
            label: label.to_string(),
        },
        None => {
            eprintln!("Unknown model: {requested}");
            list_models(current_model);
            CommandResult::Continue
        }
    }
}

fn list_models(current_model: &str) {
    println!();
    println!("{}", "Available models:".bold());

    for (id, label) in AVAILABLE_MODELS {
        let marker = if *id == current_model {
            " (active)"
        } else {
            ""
        };
        let default = if *id == DEFAULT_MODEL {
            " [default]"
        } else {
            ""
        };

        println!(
            "  {} — {}{}{}",
            id.cyan(),
            label,
            default.dimmed(),
            marker.green().bold()
        );
    }

    println!();
    println!("Usage: {}", "/model <name>".dimmed());
}
