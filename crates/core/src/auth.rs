use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use sha2::{Digest, Sha256};
use url::Url;

use crate::config::{Credentials, TokenType};

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const AUTH_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const SCOPES: &str = "org:create_api_key user:profile user:inference";

struct PkceChallenge {
    verifier: String,
    challenge: String,
    state: String,
}

fn generate_pkce() -> PkceChallenge {
    let mut rng = rand::rng();

    // Verifier: 32 random bytes → base64url (matches reference)
    let mut verifier_bytes = [0u8; 32];
    rng.fill(&mut verifier_bytes);
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);

    // Challenge: SHA-256 of verifier → base64url
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    // State: 16 random bytes → hex (reference uses .toString('hex'))
    let mut state_bytes = [0u8; 16];
    rng.fill(&mut state_bytes);
    let state = hex::encode(state_bytes);

    PkceChallenge {
        verifier,
        challenge,
        state,
    }
}

fn build_auth_url(pkce: &PkceChallenge) -> Result<String> {
    let mut url = Url::parse(AUTH_URL)?;

    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPES)
        .append_pair("state", &pkce.state)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256");

    Ok(url.to_string())
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    #[allow(dead_code)]
    token_type: Option<String>,
    #[allow(dead_code)]
    expires_in: Option<u64>,
}

/// The result of starting an OAuth flow. The caller is responsible for
/// presenting `auth_url` to the user (e.g. opening a browser) and collecting
/// the authorization code.
pub struct OAuthSession {
    pub auth_url: String,
    verifier: String,
    state: String,
}

/// Begin an OAuth flow: generates PKCE parameters and returns an
/// [`OAuthSession`] containing the URL the user must visit.
pub fn start_oauth() -> Result<OAuthSession> {
    let pkce = generate_pkce();
    let auth_url = build_auth_url(&pkce)?;

    Ok(OAuthSession {
        auth_url,
        verifier: pkce.verifier,
        state: pkce.state,
    })
}

/// Extract the authorization code from a callback URL and verify the state.
///
/// Accepts three formats:
/// - Full URL: `https://…?code=…&state=…`
/// - Code#state: `dn0Qsk…#530389…` (as shown on the callback page)
/// - Bare code: `dn0Qsk…`
pub fn parse_callback(session: &OAuthSession, input: &str) -> Result<String> {
    if input.starts_with("http") {
        let url = Url::parse(input).context("Invalid callback URL")?;

        let code = url
            .query_pairs()
            .find(|(k, _)| k == "code")
            .map(|(_, v)| v.to_string())
            .context("No 'code' parameter in callback URL")?;

        let returned_state = url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default();

        if returned_state != session.state {
            anyhow::bail!("State mismatch — possible CSRF. Please restart.");
        }

        Ok(code)
    } else if let Some((code, returned_state)) = input.split_once('#') {
        // The callback page displays "code#state"
        if returned_state != session.state {
            anyhow::bail!("State mismatch — possible CSRF. Please restart.");
        }

        Ok(code.to_string())
    } else {
        // Bare code only
        Ok(input.to_string())
    }
}

/// Exchange the authorization `code` obtained from the OAuth callback for
/// credentials. When `store_refresh` is true the returned [`Credentials`] will
/// contain the refresh token (if one was issued).
pub async fn exchange_oauth_code(
    session: &OAuthSession,
    code: &str,
    store_refresh: bool,
) -> Result<Credentials> {
    let client = reqwest::Client::new();

    // Reference script sends JSON, not form-urlencoded
    let resp = client
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "state": session.state,
            "redirect_uri": REDIRECT_URI,
            "code_verifier": session.verifier,
        }))
        .send()
        .await
        .context("Failed to exchange authorization code")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Token exchange failed ({status}): {body}");
    }

    let token_resp: TokenResponse = resp
        .json()
        .await
        .context("Failed to parse token response")?;

    if store_refresh && let Some(refresh_token) = token_resp.refresh_token {
        return Ok(Credentials {
            token: refresh_token,
            is_oauth: true,
        });
    }

    Ok(Credentials {
        token: token_resp.access_token,
        is_oauth: true,
    })
}

pub async fn refresh_access_token(creds: &Credentials) -> Result<(String, Credentials)> {
    assert_eq!(creds.token_type(), TokenType::OAuthRefresh);

    let client = reqwest::Client::new();

    let resp = client
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": creds.token,
            "client_id": CLIENT_ID,
        }))
        .send()
        .await
        .context("Failed to refresh token")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Token refresh failed ({status}): {body}");
    }

    let token_resp: TokenResponse = resp
        .json()
        .await
        .context("Failed to parse token response")?;

    let updated_creds = if let Some(new_refresh) = token_resp.refresh_token {
        Credentials {
            token: new_refresh,
            is_oauth: true,
        }
    } else {
        creds.clone()
    };

    Ok((token_resp.access_token, updated_creds))
}
