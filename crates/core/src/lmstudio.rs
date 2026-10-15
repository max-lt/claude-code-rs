//! Discovery of the models served by a local LM Studio instance.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::provider::lm_studio_base_url;

/// A probe must not stall the UI when nothing listens on the port.
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LmModel {
    /// The name LM Studio knows the model by, sent as-is on the wire.
    pub id: String,
    /// `Some` when LM Studio reported the load state (its REST API does).
    pub loaded: Option<bool>,
    /// Context length the model is loaded with, or else its maximum.
    pub context_length: Option<u64>,
}

/// Ask the server which models it serves. An error means it is unreachable or
/// answered something unexpected: the caller treats both as "not up".
///
/// Prefers LM Studio's own `/api/v0/models`, which carries the model type and
/// load state. A bare OpenAI-style `/v1/models` is the fallback.
pub async fn list_models() -> Result<Vec<LmModel>> {
    let base = lm_studio_base_url();

    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;

    let rich = client.get(format!("{base}/api/v0/models")).send().await;

    if let Ok(resp) = rich
        && resp.status().is_success()
        && let Ok(body) = resp.json::<Value>().await
    {
        return Ok(parse_models(&body));
    }

    let resp = client
        .get(format!("{base}/v1/models"))
        .send()
        .await
        .with_context(|| format!("LM Studio is not reachable at {base}"))?
        .error_for_status()
        .with_context(|| format!("LM Studio at {base} rejected the request"))?;

    let body: Value = resp.json().await.context("invalid JSON from LM Studio")?;

    Ok(parse_models(&body))
}

/// Pull chat-capable models out of a `{"data": [...]}` listing.
fn parse_models(body: &Value) -> Vec<LmModel> {
    let Some(entries) = body.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_str)?;

            // Embedding models cannot chat.
            if entry.get("type").and_then(Value::as_str) == Some("embeddings") {
                return None;
            }

            let loaded = entry
                .get("state")
                .and_then(Value::as_str)
                .map(|state| state == "loaded");

            let context_length = entry
                .get("loaded_context_length")
                .and_then(Value::as_u64)
                .or_else(|| entry.get("max_context_length").and_then(Value::as_u64));

            Some(LmModel {
                id: id.to_string(),
                loaded,
                context_length,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn embeddings_are_dropped() {
        let body = json!({ "data": [
            { "id": "chat", "type": "llm", "state": "not-loaded", "max_context_length": 4096 },
            { "id": "embed", "type": "embeddings", "state": "not-loaded" },
        ]});

        let models = parse_models(&body);

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "chat");
    }

    #[test]
    fn loaded_context_beats_the_maximum() {
        let body = json!({ "data": [{
            "id": "m",
            "state": "loaded",
            "loaded_context_length": 8192,
            "max_context_length": 262144,
        }]});

        let models = parse_models(&body);

        assert_eq!(models[0].loaded, Some(true));
        assert_eq!(models[0].context_length, Some(8192));
    }

    #[test]
    fn plain_openai_listing_has_no_state() {
        let body = json!({ "data": [{ "id": "m", "object": "model" }] });

        let models = parse_models(&body);

        assert_eq!(models[0].loaded, None);
        assert_eq!(models[0].context_length, None);
    }

    #[test]
    fn unexpected_shape_yields_nothing() {
        assert!(parse_models(&json!({ "oops": 1 })).is_empty());
    }

    /// Against a real server. Run with:
    /// `cargo test -p claude-code-core --lib -- --ignored lmstudio`
    #[tokio::test]
    #[ignore = "needs LM Studio listening on :1234"]
    async fn lmstudio_live_listing() {
        let models = list_models().await.expect("LM Studio is not up");

        assert!(!models.is_empty());
        assert!(models.iter().all(|m| !m.id.contains("embed")));
        eprintln!("{models:#?}");
    }
}
