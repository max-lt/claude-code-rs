use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::event::EventHandler;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 16384;

// Conservative limit for request payload size (Anthropic's limit is ~5MB)
const MAX_REQUEST_SIZE: usize = 4 * 1024 * 1024; // 4 MB
const MAX_TOOL_RESULT_SIZE: usize = 500_000; // 500 KB per tool result

pub const DEFAULT_MODEL: &str = "claude-sonnet-4-5";

pub const AVAILABLE_MODELS: &[(&str, &str)] = &[
    ("claude-sonnet-4-5", "Sonnet 4.5"),
    ("claude-opus-4-6", "Opus 4.6"),
    ("claude-haiku-4-5", "Haiku 4.5"),
];

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

    /// Truncate tool results in messages to prevent oversized requests
    fn truncate_tool_results(messages: &[Message]) -> Vec<Message> {
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
                                    if content.len() > MAX_TOOL_RESULT_SIZE {
                                        let truncated = format!(
                                            "{}... [truncated {} bytes]",
                                            &content[..MAX_TOOL_RESULT_SIZE],
                                            content.len() - MAX_TOOL_RESULT_SIZE
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

    fn build_request(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
        tools: Option<&[serde_json::Value]>,
    ) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .post(API_URL)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json");

        if self.is_oauth {
            req = req
                .header("authorization", format!("Bearer {}", self.access_token))
                .header("anthropic-beta", "oauth-2025-04-20");
        } else {
            req = req.header("x-api-key", &self.access_token);
        }

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
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

        req.json(&body)
    }

    pub(crate) async fn stream_message(
        &self,
        messages: &[Message],
        system_prompt: Option<&str>,
        tools: Option<&[serde_json::Value]>,
        handler: &mut dyn EventHandler,
        cancel: &CancellationToken,
    ) -> Result<StreamResult> {
        // Truncate tool results to prevent oversized requests
        let truncated_messages = Self::truncate_tool_results(messages);

        // Build the request body to check its size
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "stream": true,
            "messages": truncated_messages,
        });

        if let Some(prompt) = system_prompt {
            body["system"] = serde_json::json!(prompt);
        }

        if let Some(tools) = tools
            && !tools.is_empty()
        {
            body["tools"] = serde_json::json!(tools);
        }

        // Check request size
        let body_json = serde_json::to_string(&body)?;
        let body_size = body_json.len();

        if body_size > MAX_REQUEST_SIZE {
            anyhow::bail!(
                "Request too large ({} MB). The conversation history is too long. \
                 Please use /clear to start a new conversation.",
                body_size / (1024 * 1024)
            );
        }

        let request = self.build_request(&truncated_messages, system_prompt, tools);
        let mut es = EventSource::new(request).context("Failed to create event source")?;

        let mut state = StreamState::new();

        loop {
            tokio::select! {
                event = es.next() => {
                    let Some(event) = event else { break };

                    match event {
                        Ok(Event::Open) => {}
                        Ok(Event::Message(msg)) => {
                            let done = handle_sse_event(&msg.event, &msg.data, &mut state, handler)?;

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

        Ok(state.into_result())
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

    #[test]
    fn test_truncate_tool_results() {
        let large_content = "x".repeat(MAX_TOOL_RESULT_SIZE + 1000);

        let messages = vec![Message {
            role: "user".to_string(),
            content: Content::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "test".to_string(),
                content: large_content.clone(),
                is_error: Some(false),
            }]),
        }];

        let truncated = ApiClient::truncate_tool_results(&messages);

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
}
