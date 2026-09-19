use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::event::EventHandler;
use crate::openai;
use crate::provider::Provider;

const API_VERSION: &str = "2023-06-01";

const CEREBRAS_KEY_VAR: &str = "CEREBRAS_API_KEY";

pub use crate::provider::{AVAILABLE_MODELS, DEFAULT_MODEL};

// ---------------------------------------------------------------------------
// Content model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }

    pub fn blocks(blocks: Vec<ContentBlock>) -> Self {
        Self::Blocks(blocks)
    }

    /// Extract the concatenated plain text from this content.
    pub fn to_text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Content,
}

#[derive(Debug, Clone, Copy)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
}

pub struct StreamResult {
    pub content: Vec<ContentBlock>,
    pub usage: Usage,
    pub stop_reason: StopReason,
}

// ---------------------------------------------------------------------------
// Stream state (tracks the block currently being built)
// ---------------------------------------------------------------------------

enum BlockKind {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
}

struct StreamState {
    blocks: Vec<ContentBlock>,
    current: Option<BlockKind>,
    usage: Usage,
    stop_reason: StopReason,
}

impl StreamState {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            current: None,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
            },
            stop_reason: StopReason::EndTurn,
        }
    }

    fn start_block(&mut self, parsed: &serde_json::Value) {
        let block_type = parsed
            .get("content_block")
            .and_then(|b| b.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("");

        self.current = match block_type {
            "text" => Some(BlockKind::Text {
                text: String::new(),
            }),
            "tool_use" => {
                let block = &parsed["content_block"];
                let id = block["id"].as_str().unwrap_or("").to_string();
                let name = block["name"].as_str().unwrap_or("").to_string();

                Some(BlockKind::ToolUse {
                    id,
                    name,
                    json: String::new(),
                })
            }
            _ => None,
        };
    }

    fn apply_delta(&mut self, parsed: &serde_json::Value, handler: &mut dyn EventHandler) {
        let delta = match parsed.get("delta") {
            Some(d) => d,
            None => return,
        };

        let delta_type = delta.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match (&mut self.current, delta_type) {
            (Some(BlockKind::Text { text }), "text_delta") => {
                if let Some(chunk) = delta.get("text").and_then(|t| t.as_str()) {
                    handler.on_text(chunk);
                    text.push_str(chunk);
                }
            }
            (Some(BlockKind::ToolUse { json, .. }), "input_json_delta") => {
                if let Some(chunk) = delta.get("partial_json").and_then(|t| t.as_str()) {
                    json.push_str(chunk);
                }
            }
            _ => {}
        }
    }

    fn finish_block(&mut self) {
        let block = match self.current.take() {
            Some(b) => b,
            None => return,
        };

        match block {
            BlockKind::Text { text } => {
                self.blocks.push(ContentBlock::Text { text });
            }
            BlockKind::ToolUse { id, name, json } => {
                let input = serde_json::from_str(&json)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                self.blocks.push(ContentBlock::ToolUse { id, name, input });
            }
        }
    }

    fn into_result(self) -> StreamResult {
        StreamResult {
            content: self.blocks,
            usage: self.usage,
            stop_reason: self.stop_reason,
        }
    }
}

// ---------------------------------------------------------------------------
// API client
// ---------------------------------------------------------------------------

pub(crate) struct ApiClient {
    client: reqwest::Client,
    access_token: String,
    is_oauth: bool,
    model: String,
}

impl ApiClient {
    pub(crate) fn new(access_token: String, is_oauth: bool) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .expect("failed to build HTTP client");

        Self {
            client,
            access_token,
            is_oauth,
            model: DEFAULT_MODEL.to_string(),
        }
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub(crate) fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub(crate) fn provider(&self) -> Provider {
        Provider::for_model(&self.model)
    }

    /// Truncate tool results in messages to prevent oversized requests
    fn truncate_tool_results(messages: &[Message], limit: usize) -> Vec<Message> {
        messages
            .iter()
            .map(|msg| {
                let content = match &msg.content {
                    Content::Blocks(blocks) => {
                        let truncated_blocks: Vec<ContentBlock> = blocks
                            .iter()
                            .map(|block| match block {
                                ContentBlock::ToolResult {
                                    tool_use_id,
                                    content,
                                    is_error,
                                } => {
                                    if content.len() > limit {
                                        // Never split a UTF-8 sequence.
                                        let mut end = limit;
                                        while end > 0 && !content.is_char_boundary(end) {
                                            end -= 1;
                                        }

                                        let truncated = format!(
                                            "{}... [truncated {} bytes]",
                                            &content[..end],
                                            content.len() - end
                                        );

                                        ContentBlock::ToolResult {
                                            tool_use_id: tool_use_id.clone(),
                                            content: truncated,
                                            is_error: *is_error,
                                        }
                                    } else {
                                        block.clone()
                                    }
                                }
                                _ => block.clone(),
                            })
                            .collect();

                        Content::Blocks(truncated_blocks)
                    }
                    _ => msg.content.clone(),
                };

                Message {
                    role: msg.role.clone(),
                    content,
                }
            })
            .collect()
    }

    /// Serialize the request body in the provider's own format.
    fn build_body(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
        tools: Option<&[serde_json::Value]>,
    ) -> serde_json::Value {
        let provider = self.provider();

        if provider == Provider::Cerebras {
            return openai::build_body(
                &self.model,
                messages,
                system_prompt,
                tools,
                provider.max_output_tokens(),
            );
        }

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": provider.max_output_tokens(),
            "stream": true,
            "messages": messages,
        });

        if let Some(prompt) = system_prompt {
            body["system"] = serde_json::json!(prompt);
        }

        if let Some(tools) = tools
            && !tools.is_empty()
        {
            body["tools"] = serde_json::json!(tools);
        }

        body
    }

    /// Apply the provider's endpoint and authentication.
    fn build_request(&self, body: &serde_json::Value) -> Result<reqwest::RequestBuilder> {
        let provider = self.provider();

        let mut req = self
            .client
            .post(provider.endpoint())
            .header("content-type", "application/json");

        match provider {
            Provider::Anthropic => {
                req = req.header("anthropic-version", API_VERSION);

                if self.is_oauth {
                    req = req
                        .header("authorization", format!("Bearer {}", self.access_token))
                        .header("anthropic-beta", "oauth-2025-04-20");
                } else {
                    req = req.header("x-api-key", &self.access_token);
                }
            }
            Provider::Cerebras => {
                let key = std::env::var(CEREBRAS_KEY_VAR).map_err(|_| {
                    anyhow::anyhow!(
                        "{CEREBRAS_KEY_VAR} is not set. Add it to .env or export it \
                         to use {}.",
                        self.model
                    )
                })?;

                req = req.header("authorization", format!("Bearer {key}"));
            }
        }

        Ok(req.json(body))
    }

    pub(crate) async fn stream_message(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
        tools: Option<&[serde_json::Value]>,
        handler: &mut dyn EventHandler,
        cancel: &CancellationToken,
    ) -> Result<StreamResult> {
        let provider = self.provider();

        // Truncate tool results to prevent oversized requests
        let truncated_messages =
            Self::truncate_tool_results(messages, provider.max_tool_result_bytes());

        let body = self.build_body(&truncated_messages, system_prompt, tools);

        // Check request size
        let body_size = serde_json::to_string(&body)?.len();
        let limit = provider.max_request_bytes();

        if body_size > limit {
            anyhow::bail!(
                "Request too large ({} KB, limit {} KB for {}). The conversation \
                 history is too long. Please use /clear to start a new conversation.",
                body_size / 1024,
                limit / 1024,
                self.model,
            );
        }

        let request = self.build_request(&body)?;
        let mut es = EventSource::new(request).context("Failed to create event source")?;

        let mut state = StreamState::new();
        let mut chunks = openai::Accumulator::default();

        loop {
            tokio::select! {
                event = es.next() => {
                    let Some(event) = event else { break };

                    match event {
                        Ok(Event::Open) => {}
                        Ok(Event::Message(msg)) => {
                            // Cerebras sends unnamed events carrying one chat
                            // completion chunk each; Anthropic names them.
                            let done = match provider {
                                Provider::Anthropic => {
                                    handle_sse_event(&msg.event, &msg.data, &mut state, handler)?
                                }
                                Provider::Cerebras => chunks.handle_chunk(&msg.data, handler)?,
                            };

                            if done {
                                es.close();
                                break;
                            }
                        }
                        Err(reqwest_eventsource::Error::StreamEnded) => break,
                        Err(e) => {
                            es.close();

                            // Better error messages for common cases
                            let err_str = e.to_string();

                            if err_str.contains("400") || err_str.contains("Bad Request") {
                                anyhow::bail!(
                                    "API request rejected (400 Bad Request). The request may be too large. \
                                     Try using /clear to start a new conversation."
                                );
                            }

                            anyhow::bail!("Stream error: {e}");
                        }
                    }
                }

                () = cancel.cancelled() => {
                    es.close();
                    anyhow::bail!("Cancelled");
                }
            }
        }

        Ok(match provider {
            Provider::Anthropic => state.into_result(),
            Provider::Cerebras => chunks.into_result(),
        })
    }
}

fn handle_sse_event(
    event_type: &str,
    data: &str,
    state: &mut StreamState,
    handler: &mut dyn EventHandler,
) -> Result<bool> {
    match event_type {
        "message_start" => {
            let parsed: serde_json::Value = serde_json::from_str(data)?;

            if let Some(u) = parsed.get("message").and_then(|m| m.get("usage"))
                && let Some(input) = u.get("input_tokens").and_then(|v| v.as_u64())
            {
                state.usage.input_tokens = input;
            }
        }
        "content_block_start" => {
            let parsed: serde_json::Value = serde_json::from_str(data)?;
            state.start_block(&parsed);
        }
        "content_block_delta" => {
            let parsed: serde_json::Value = serde_json::from_str(data)?;
            state.apply_delta(&parsed, handler);
        }
        "content_block_stop" => {
            state.finish_block();
        }
        "message_delta" => {
            let parsed: serde_json::Value = serde_json::from_str(data)?;

            if let Some(u) = parsed.get("usage")
                && let Some(output) = u.get("output_tokens").and_then(|v| v.as_u64())
            {
                state.usage.output_tokens = output;
            }

            if let Some(reason) = parsed
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|r| r.as_str())
            {
                state.stop_reason = match reason {
                    "tool_use" => StopReason::ToolUse,
                    "max_tokens" => StopReason::MaxTokens,
                    _ => StopReason::EndTurn,
                };
            }
        }
        "message_stop" => {
            return Ok(true);
        }
        "error" => {
            let parsed: serde_json::Value = serde_json::from_str(data)?;
            let msg = parsed
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            handler.on_error(msg);
            return Ok(true); // Stop stream on error
        }
        "ping" => {}
        _ => {}
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_LIMIT: usize = 500_000;

    /// The Anthropic body must keep its original shape: a top-level `system`,
    /// raw tool definitions, and `max_tokens`.
    #[test]
    fn anthropic_body_is_unchanged_by_provider_routing() {
        let client = ApiClient::new("token".to_string(), true);
        assert_eq!(client.provider(), Provider::Anthropic);

        let messages = vec![Message {
            role: "user".to_string(),
            content: Content::text("hi"),
        }];

        let tools = vec![serde_json::json!({
            "name": "Read",
            "description": "Read a file",
            "input_schema": { "type": "object" },
        })];

        let body = client.build_body(&messages, Some("be brief"), Some(&tools));

        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["max_tokens"], 16384);
        assert_eq!(body["stream"], true);
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["messages"][0]["content"], "hi");

        // Anthropic takes the tool definitions as-is.
        assert_eq!(body["tools"][0]["name"], "Read");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert!(body.get("max_completion_tokens").is_none());
    }

    #[test]
    fn switching_model_switches_the_wire_format() {
        let mut client = ApiClient::new("token".to_string(), true);
        client.set_model("qwen-3.8-27b".to_string());

        let body = client.build_body(&[], Some("be brief"), None);

        assert_eq!(body["max_completion_tokens"], 8192);
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(body.get("system").is_none());
        assert!(body.get("max_tokens").is_none());
    }

    /// One real round-trip against Cerebras, covering the whole path: body
    /// translation, HTTP, SSE chunks, and accumulation back into blocks.
    ///
    /// Needs `CEREBRAS_API_KEY`. Run with:
    /// `cargo test -p claude-code-core --lib -- --ignored cerebras`
    #[tokio::test]
    #[ignore = "calls the live Cerebras API"]
    async fn cerebras_round_trip_produces_a_tool_use_block() {
        #[derive(Default)]
        struct Collect {
            text: String,
            thinking: String,
            error: Option<String>,
        }

        impl EventHandler for Collect {
            fn on_text(&mut self, text: &str) {
                self.text.push_str(text);
            }

            fn on_thinking(&mut self, text: &str) {
                self.thinking.push_str(text);
            }

            fn on_error(&mut self, message: &str) {
                self.error = Some(message.to_string());
            }
        }

        let mut client = ApiClient::new(String::new(), false);
        client.set_model("qwen-3.8-27b".to_string());
        assert_eq!(client.provider(), Provider::Cerebras);

        let messages = vec![Message {
            role: "user".to_string(),
            content: Content::text("List the files in /tmp. Use the Bash tool."),
        }];

        let tools = vec![serde_json::json!({
            "name": "Bash",
            "description": "Run a shell command",
            "input_schema": {
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"],
            },
        })];

        let mut handler = Collect::default();

        let result = client
            .stream_message(
                &messages,
                Some("You are a terminal assistant."),
                Some(&tools),
                &mut handler,
                &CancellationToken::new(),
            )
            .await
            .expect("stream failed");

        assert_eq!(handler.error, None);
        assert_eq!(result.stop_reason, StopReason::ToolUse);
        assert!(result.usage.input_tokens > 0, "no input tokens reported");
        assert!(result.usage.output_tokens > 0, "no output tokens reported");

        let call = result
            .content
            .iter()
            .find_map(|b| match b {
                ContentBlock::ToolUse { name, input, .. } => Some((name, input)),
                _ => None,
            })
            .expect("no tool_use block in the response");

        assert_eq!(call.0, "Bash");
        assert!(
            call.1.get("command").and_then(|c| c.as_str()).is_some(),
            "tool input did not parse into an object with a command: {:?}",
            call.1
        );
    }

    #[test]
    fn test_truncate_tool_results() {
        let large_content = "x".repeat(TEST_LIMIT + 1000);

        let messages = vec![Message {
            role: "user".to_string(),
            content: Content::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "test".to_string(),
                content: large_content.clone(),
                is_error: Some(false),
            }]),
        }];

        let truncated = ApiClient::truncate_tool_results(&messages, TEST_LIMIT);

        match &truncated[0].content {
            Content::Blocks(blocks) => match &blocks[0] {
                ContentBlock::ToolResult { content, .. } => {
                    assert!(content.len() < large_content.len());
                    assert!(content.contains("[truncated"));
                }
                _ => panic!("Expected ToolResult"),
            },
            _ => panic!("Expected Blocks"),
        }
    }

    #[test]
    fn truncation_never_splits_a_utf8_sequence() {
        // A 3-byte character straddling the cut point.
        let messages = vec![Message {
            role: "user".to_string(),
            content: Content::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "t".to_string(),
                content: "é".repeat(200),
                is_error: None,
            }]),
        }];

        // The limit lands mid-character: 5 is inside the third 'é'.
        let truncated = ApiClient::truncate_tool_results(&messages, 5);

        match &truncated[0].content {
            Content::Blocks(blocks) => match &blocks[0] {
                ContentBlock::ToolResult { content, .. } => {
                    assert!(content.starts_with("éé"));
                    assert!(content.contains("[truncated"));
                }
                _ => panic!("Expected ToolResult"),
            },
            _ => panic!("Expected Blocks"),
        }
    }
}
