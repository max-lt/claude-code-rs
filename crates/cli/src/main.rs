mod commands;
mod permissions;
mod tui;
mod ui;

use anyhow::Result;

use claude_code_core::config::{Credentials, TokenType};
use claude_code_core::session::SessionBuilder;
use claude_code_core::{auth, config};

use permissions::ChannelPermissions;

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
            println!("Refreshing access token...");
            let (access_token, updated_creds) = auth::refresh_access_token(creds).await?;
            Ok((access_token, true, Some(updated_creds)))
        }
        TokenType::ApiKey => Ok((creds.token.clone(), false, None)),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("claude-code-rs v0.1.0\n");

    let creds = match config::load_credentials()? {
        Some(c) => {
            println!("Loaded saved credentials.");
            c
        }
        None => {
            let c = login().await?;
            config::save_credentials(&c)?;
            println!("Credentials saved.");
            c
        }
    };

    let (access_token, is_oauth, updated_creds) = get_access_token(&creds).await?;

    if let Some(new_creds) = updated_creds {
        config::save_credentials(&new_creds)?;
    }

    let cwd = std::env::current_dir()?;
    let settings = config::load_settings(&cwd);

    let (ui_tx, ui_rx) = tokio::sync::mpsc::unbounded_channel();
    let perms = ChannelPermissions::new(settings.permissions, cwd, ui_tx.clone());

    let session = SessionBuilder::new(access_token, is_oauth).permissions(perms)?;

    tui::run(session, ui_tx, ui_rx)
}
