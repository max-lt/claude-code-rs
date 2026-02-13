mod commands;
mod permissions;
mod ui;

use anyhow::Result;
use colored::Colorize;

use claude_code_core::config::{Credentials, TokenType};
use claude_code_core::event::EventHandler;
use claude_code_core::session::SessionBuilder;
use claude_code_core::{auth, config};

use commands::CommandResult;
use permissions::InteractivePermissions;

struct CliEventHandler;

impl EventHandler for CliEventHandler {
    fn on_text(&mut self, text: &str) {
        print!("{text}");
    }

    fn on_error(&mut self, message: &str) {
        eprintln!("\n{}: {message}", "API error".red());
    }

    fn on_tool_use_start(&mut self, name: &str, _id: &str) {
        println!("\n{} {}", "Tool:".yellow().bold(), name.bold());
    }

    fn on_tool_use_end(&mut self, _name: &str) {}

    fn on_tool_executing(&mut self, _name: &str, input: &serde_json::Value) {
        let display = match input.get("command").and_then(|c| c.as_str()) {
            Some(cmd) => cmd.to_string(),
            None => serde_json::to_string_pretty(input).unwrap_or_default(),
        };

        println!("{}", display.dimmed());
    }

    fn on_tool_result(&mut self, _name: &str, output: &str, is_error: bool) {
        const MAX_LINES: usize = 5;

        let lines: Vec<&str> = output.lines().collect();
        let total_lines = lines.len();

        let display = if total_lines > MAX_LINES {
            let preview: String = lines[..MAX_LINES].join("\n");
            format!("{preview}\n... ({total_lines} lines total)")
        } else {
            output.to_string()
        };

        if is_error {
            eprintln!("{}", display.red());
        } else {
            println!("{}", display.dimmed());
        }
    }
}

async fn login() -> Result<Credentials> {
    let method = ui::prompt_login_method()?;

    match method {
        ui::LoginMethod::OAuth => {
            let store_refresh = ui::prompt_store_refresh()?;
            let session = auth::start_oauth()?;

            println!("Opening browser for authentication...");

            if webbrowser::open(&session.auth_url).is_err() {
                println!("Could not open browser. Please visit this URL manually:");
                println!("{}", session.auth_url);
            }

            let input = ui::prompt_oauth_code()?;
            let code = auth::parse_callback(&session, &input)?;
            auth::exchange_oauth_code(&session, &code, store_refresh).await
        }
        ui::LoginMethod::ApiKey => {
            let key = ui::prompt_api_key()?;
            Ok(Credentials {
                token: key,
                is_oauth: false,
            })
        }
    }
}

async fn get_access_token(creds: &Credentials) -> Result<(String, bool, Option<Credentials>)> {
    match creds.token_type() {
        TokenType::OAuthAccess => Ok((creds.token.clone(), true, None)),
        TokenType::OAuthRefresh => {
            println!("{}", "Refreshing access token...".dimmed());
            let (access_token, updated_creds) = auth::refresh_access_token(creds).await?;
            Ok((access_token, true, Some(updated_creds)))
        }
        TokenType::ApiKey => Ok((creds.token.clone(), false, None)),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    ui::print_welcome();

    let creds = match config::load_credentials()? {
        Some(c) => {
            println!("{}", "Loaded saved credentials.".dimmed());
            c
        }
        None => {
            let c = login().await?;
            config::save_credentials(&c)?;
            println!("{}", "Credentials saved.".dimmed());
            c
        }
    };

    let (access_token, is_oauth, updated_creds) = get_access_token(&creds).await?;

    if let Some(new_creds) = updated_creds {
        config::save_credentials(&new_creds)?;
    }

    let cwd = std::env::current_dir()?;
    let settings = config::load_settings(&cwd);
    let perms = InteractivePermissions::new(settings.permissions, cwd);

    let mut session = SessionBuilder::new(access_token, is_oauth).permissions(perms)?;
    let mut handler = CliEventHandler;

    loop {
        let input = match ui::read_user_input()? {
            Some(text) => text,
            None => continue,
        };

        // Try slash commands first
        if let Some(result) = commands::handle_command(&input, session.model()) {
            match result {
                CommandResult::Exit => break,
                CommandResult::SetModel { id, label } => {
                    session.set_model(id);
                    println!("{}", format!("Switched to {label}.").dimmed());
                    continue;
                }
                CommandResult::Continue => {
                    if input == "/clear" {
                        session.clear();
                    }

                    continue;
                }
            }
        }

        println!();

        match session.send_message(&input, &mut handler).await {
            Ok(usage) => {
                println!();
                println!(
                    "{}",
                    format!(
                        "[tokens: {} in, {} out]",
                        usage.input_tokens, usage.output_tokens
                    )
                    .dimmed()
                );
            }
            Err(e) => {
                eprintln!("{}: {e}", "Error".red());
            }
        }

        println!();
    }

    Ok(())
}
