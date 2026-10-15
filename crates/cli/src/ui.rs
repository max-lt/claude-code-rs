use anyhow::Result;
use claude_code_core::history::SessionSummary;
use dialoguer::{Input, Password, Select};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoginMethod {
    OAuth,
    ApiKey,
}

pub fn prompt_login_method() -> Result<LoginMethod> {
    let items = &["Login with OAuth (browser)", "Enter API key"];

    let selection = Select::new()
        .with_prompt("How would you like to authenticate?")
        .items(items)
        .default(0)
        .interact()?;

    match selection {
        0 => Ok(LoginMethod::OAuth),
        _ => Ok(LoginMethod::ApiKey),
    }
}

pub fn prompt_store_refresh() -> Result<bool> {
    let items = &[
        "Store refresh token (persistent login)",
        "Store access token only (~8 hours)",
    ];

    let selection = Select::new()
        .with_prompt("How should we store your credentials?")
        .items(items)
        .default(0)
        .interact()?;

    Ok(selection == 0)
}

pub fn prompt_api_key() -> Result<String> {
    let key = Password::new()
        .with_prompt("Enter your Anthropic API key")
        .interact()?;
    Ok(key)
}

/// "5m ago", "3h ago", "2d ago".
fn format_age(updated_at: u64) -> String {
    let secs = claude_code_core::history::now_secs().saturating_sub(updated_at);

    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

/// Let the user pick a saved session. `None` means "start a new one".
pub fn prompt_resume(saved: &[SessionSummary]) -> Result<Option<usize>> {
    let mut items: Vec<String> = saved
        .iter()
        .map(|s| {
            format!(
                "{:>8}  {:>3} msgs  {}",
                format_age(s.updated_at),
                s.message_count,
                s.preview
            )
        })
        .collect();

    items.push("Start a new conversation".to_string());

    let selection = Select::new()
        .with_prompt("Resume which conversation?")
        .items(&items)
        .default(0)
        .interact()?;

    Ok((selection < saved.len()).then_some(selection))
}

pub fn prompt_oauth_code() -> Result<String> {
    let code = Input::new()
        .with_prompt("Paste the complete callback URL (or just the code)")
        .interact_text()?;
    Ok(code)
}
