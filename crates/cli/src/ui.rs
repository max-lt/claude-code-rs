use anyhow::Result;
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

pub fn prompt_oauth_code() -> Result<String> {
    let code = Input::new()
        .with_prompt("Paste the complete callback URL (or just the code)")
        .interact_text()?;
    Ok(code)
}
