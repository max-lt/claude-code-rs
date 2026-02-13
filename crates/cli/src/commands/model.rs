use claude_code_core::api::{AVAILABLE_MODELS, DEFAULT_MODEL};

use super::CommandResult;

pub fn run(args: &str, current_model: &str) -> CommandResult {
    let requested = args.trim();

    if requested.is_empty() {
        return CommandResult::Info(list_models(current_model));
    }

    // Try exact match first, then substring match
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
        None => CommandResult::Info(format!(
            "Unknown model: {requested}\n{}",
            list_models(current_model)
        )),
    }
}

fn list_models(current_model: &str) -> String {
    let mut text = String::from("Available models:\n");

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

        text.push_str(&format!("  {id} — {label}{default}{marker}\n"));
    }

    text.push_str("\nUsage: /model <name>");
    text
}
