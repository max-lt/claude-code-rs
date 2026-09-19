//! Translation to and from the OpenAI chat completions wire format.
//!
//! `ccrs` speaks Anthropic internally: a turn is a [`Message`] holding either
//! text or a list of [`ContentBlock`]s. This module converts that to the shape
//! Cerebras expects, and folds its SSE chunks back into a [`StreamResult`].
//!
//! The two formats differ in three ways that matter:
//!
//! - The system prompt is a `system` message, not a top-level field.
//! - A tool result is its own `tool` message. Anthropic batches every result
//!   of a turn into one `user` message, so results must be fanned out.
//! - Tool arguments arrive as a JSON *string*, assembled from fragments.

use anyhow::Result;
use serde_json::{Value, json};

use crate::api::{Content, ContentBlock, Message, StopReason, StreamResult, Usage};
use crate::event::EventHandler;

/// Build a chat completions request body.
pub fn build_body(
    model: &str,
    messages: &[Message],
    system_prompt: Option<&str>,
    tools: Option<&[Value]>,
    max_output_tokens: u32,
) -> Value {
    let mut wire = Vec::new();

    if let Some(prompt) = system_prompt {
        wire.push(json!({ "role": "system", "content": prompt }));
    }

    for msg in messages {
        push_message(&mut wire, msg);
    }

    let mut body = json!({
        "model": model,
        "stream": true,
        // OpenAI's `max_tokens` is deprecated and capped differently.
        "max_completion_tokens": max_output_tokens,
        "messages": wire,
    });

    if let Some(tools) = tools
        && !tools.is_empty()
    {
        body["tools"] = json!(tools.iter().map(convert_tool).collect::<Vec<_>>());
    }

    body
}

/// Convert one internal turn, appending one or more wire messages.
fn push_message(wire: &mut Vec<Value>, msg: &Message) {
    let blocks = match &msg.content {
        Content::Text(text) => {
            wire.push(json!({ "role": msg.role, "content": text }));
            return;
        }
        Content::Blocks(blocks) => blocks,
    };

    if msg.role == "assistant" {
        let mut text = String::new();
        let mut tool_calls = Vec::new();

        for block in blocks {
            match block {
                ContentBlock::Text { text: chunk } => text.push_str(chunk),
                ContentBlock::ToolUse { id, name, input } => tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": input.to_string() },
                })),
                ContentBlock::ToolResult { .. } => {}
            }
        }

        let mut out = json!({ "role": "assistant" });

        // A tool-only turn carries no text; the field must still be present.
        out["content"] = if text.is_empty() && !tool_calls.is_empty() {
            Value::Null
        } else {
            json!(text)
        };

        if !tool_calls.is_empty() {
            out["tool_calls"] = json!(tool_calls);
        }

        wire.push(out);
        return;
    }

    // A user turn carrying tool results. Each result is its own message, and
    // they must directly follow the assistant turn that requested them.
    let mut text = String::new();

    for block in blocks {
        match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => wire.push(json!({
                "role": "tool",
                "tool_call_id": tool_use_id,
                "content": content,
            })),
            ContentBlock::Text { text: chunk } => text.push_str(chunk),
            ContentBlock::ToolUse { .. } => {}
        }
    }

    if !text.is_empty() {
        wire.push(json!({ "role": msg.role, "content": text }));
    }
}

/// `{name, description, input_schema}` becomes a `function` tool.
fn convert_tool(tool: &Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.get("name").cloned().unwrap_or(Value::Null),
            "description": tool.get("description").cloned().unwrap_or(Value::Null),
            "parameters": tool.get("input_schema").cloned().unwrap_or(json!({
                "type": "object",
                "properties": {},
            })),
        },
    })
}

// ---------------------------------------------------------------------------
// Stream accumulation
// ---------------------------------------------------------------------------

/// Upper bound on tool calls in one turn, to bound the `index` allocation.
const MAX_TOOL_CALLS: usize = 128;

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

/// Folds chat completion chunks into a single [`StreamResult`].
pub struct Accumulator {
    text: String,
    calls: Vec<PendingCall>,
    usage: Usage,
    stop_reason: StopReason,
}

impl Default for Accumulator {
    fn default() -> Self {
        Self {
            text: String::new(),
            calls: Vec::new(),
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
            },
            stop_reason: StopReason::EndTurn,
        }
    }
}

impl Accumulator {
    /// Consume one `data:` payload. Returns true when the stream is over.
    pub fn handle_chunk(&mut self, data: &str, handler: &mut dyn EventHandler) -> Result<bool> {
        if data.trim() == "[DONE]" {
            return Ok(true);
        }

        let parsed: Value = serde_json::from_str(data)?;

        if let Some(message) = parsed
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            handler.on_error(message);
            return Ok(true);
        }

        // Usage rides on the final chunk.
        if let Some(usage) = parsed.get("usage") {
            if let Some(n) = usage.get("prompt_tokens").and_then(Value::as_u64) {
                self.usage.input_tokens = n;
            }

            if let Some(n) = usage.get("completion_tokens").and_then(Value::as_u64) {
                self.usage.output_tokens = n;
            }
        }

        let Some(choice) = parsed
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        else {
            return Ok(false);
        };

        if let Some(delta) = choice.get("delta") {
            if let Some(chunk) = delta.get("content").and_then(Value::as_str) {
                handler.on_text(chunk);
                self.text.push_str(chunk);
            }

            // Qwen streams its chain of thought in a field of its own.
            if let Some(chunk) = delta.get("reasoning").and_then(Value::as_str) {
                handler.on_thinking(chunk);
            }

            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    self.apply_tool_call(call);
                }
            }
        }

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = match reason {
                "tool_calls" | "function_call" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            };
        }

        Ok(false)
    }

    /// Merge one tool call fragment. Cerebras sends the arguments whole, but
    /// the format allows them to be split across chunks, so append.
    fn apply_tool_call(&mut self, call: &Value) {
        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0);

        // `index` sizes an allocation, so refuse an absurd one outright.
        if index >= MAX_TOOL_CALLS as u64 {
            return;
        }

        let index = index as usize;

        if self.calls.len() <= index {
            self.calls.resize_with(index + 1, PendingCall::default);
        }

        let pending = &mut self.calls[index];

        if let Some(id) = call.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            pending.id = id.to_string();
        }

        let Some(function) = call.get("function") else {
            return;
        };

        if let Some(name) = function.get("name").and_then(Value::as_str)
            && !name.is_empty()
        {
            pending.name = name.to_string();
        }

        if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
            pending.arguments.push_str(arguments);
        }
    }

    pub fn into_result(self) -> StreamResult {
        let mut content = Vec::new();

        if !self.text.is_empty() {
            content.push(ContentBlock::Text { text: self.text });
        }

        for (index, call) in self.calls.into_iter().enumerate() {
            if call.name.is_empty() {
                continue;
            }

            let input = serde_json::from_str(&call.arguments)
                .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));

            // An id is required to match the result back; synthesize one if
            // the provider omitted it.
            let id = if call.id.is_empty() {
                format!("call_{index}")
            } else {
                call.id
            };

            content.push(ContentBlock::ToolUse {
                id,
                name: call.name,
                input,
            });
        }

        StreamResult {
            content,
            usage: self.usage,
            stop_reason: self.stop_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Silent;

    impl EventHandler for Silent {
        fn on_text(&mut self, _text: &str) {}
        fn on_error(&mut self, _message: &str) {}
    }

    fn assistant_tool_use() -> Message {
        Message {
            role: "assistant".to_string(),
            content: Content::blocks(vec![ContentBlock::ToolUse {
                id: "abc".to_string(),
                name: "Bash".to_string(),
                input: json!({ "command": "ls" }),
            }]),
        }
    }

    #[test]
    fn system_prompt_becomes_a_message() {
        let body = build_body("qwen-3.8-27b", &[], Some("be brief"), None, 8192);
        let messages = body["messages"].as_array().unwrap();

        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "be brief");
        assert_eq!(body["max_completion_tokens"], 8192);
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn tool_use_becomes_tool_calls_with_string_arguments() {
        let body = build_body("qwen-3.8-27b", &[assistant_tool_use()], None, None, 8192);
        let message = &body["messages"][0];

        assert_eq!(message["role"], "assistant");
        assert!(message["content"].is_null());

        let call = &message["tool_calls"][0];
        assert_eq!(call["id"], "abc");
        assert_eq!(call["type"], "function");
        assert_eq!(call["function"]["name"], "Bash");

        // Arguments are a JSON string, not an object.
        let arguments = call["function"]["arguments"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(arguments).unwrap(),
            json!({ "command": "ls" })
        );
    }

    #[test]
    fn batched_tool_results_fan_out_into_one_message_each() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: Content::blocks(vec![
                ContentBlock::ToolResult {
                    tool_use_id: "a".to_string(),
                    content: "first".to_string(),
                    is_error: None,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "b".to_string(),
                    content: "second".to_string(),
                    is_error: Some(true),
                },
            ]),
        }];

        let body = build_body("qwen-3.8-27b", &messages, None, None, 8192);
        let wire = body["messages"].as_array().unwrap();

        assert_eq!(wire.len(), 2);
        assert_eq!(wire[0]["role"], "tool");
        assert_eq!(wire[0]["tool_call_id"], "a");
        assert_eq!(wire[0]["content"], "first");
        assert_eq!(wire[1]["tool_call_id"], "b");
    }

    #[test]
    fn tool_results_precede_trailing_user_text() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: Content::blocks(vec![
                ContentBlock::Text {
                    text: "and then?".to_string(),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "a".to_string(),
                    content: "done".to_string(),
                    is_error: None,
                },
            ]),
        }];

        let body = build_body("qwen-3.8-27b", &messages, None, None, 8192);
        let wire = body["messages"].as_array().unwrap();

        assert_eq!(wire[0]["role"], "tool");
        assert_eq!(wire[1]["role"], "user");
    }

    #[test]
    fn tool_schema_is_renamed_to_parameters() {
        let tools = vec![json!({
            "name": "Read",
            "description": "Read a file",
            "input_schema": { "type": "object", "properties": {} },
        })];

        let body = build_body("qwen-3.8-27b", &[], None, Some(&tools), 8192);
        let tool = &body["tools"][0];

        assert_eq!(tool["type"], "function");
        assert_eq!(tool["function"]["name"], "Read");
        assert_eq!(tool["function"]["parameters"]["type"], "object");
        assert!(tool["function"].get("input_schema").is_none());
    }

    #[test]
    fn accumulates_text_and_a_split_tool_call() {
        let mut acc = Accumulator::default();
        let mut handler = Silent;

        for chunk in [
            r#"{"choices":[{"delta":{"content":"working"},"index":0}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x1","function":{"name":"Bash","arguments":"{\"com"}}]},"index":0}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"mand\": \"ls\"}"}}]},"index":0}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":4}}"#,
        ] {
            assert!(!acc.handle_chunk(chunk, &mut handler).unwrap());
        }

        let result = acc.into_result();

        assert_eq!(result.stop_reason, StopReason::ToolUse);
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.usage.output_tokens, 4);

        match &result.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "working"),
            other => panic!("expected text, got {other:?}"),
        }

        match &result.content[1] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "x1");
                assert_eq!(name, "Bash");
                assert_eq!(input, &json!({ "command": "ls" }));
            }
            other => panic!("expected tool use, got {other:?}"),
        }
    }

    #[test]
    fn done_sentinel_ends_the_stream() {
        let mut acc = Accumulator::default();
        assert!(acc.handle_chunk("[DONE]", &mut Silent).unwrap());
    }

    #[test]
    fn malformed_arguments_yield_an_empty_object() {
        let mut acc = Accumulator::default();

        acc.handle_chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","function":{"name":"Bash","arguments":"{not json"}}]},"index":0}]}"#,
            &mut Silent,
        )
        .unwrap();

        match &acc.into_result().content[0] {
            ContentBlock::ToolUse { input, .. } => assert_eq!(input, &json!({})),
            other => panic!("expected tool use, got {other:?}"),
        }
    }

    #[test]
    fn finish_reason_length_maps_to_max_tokens() {
        let mut acc = Accumulator::default();

        acc.handle_chunk(
            r#"{"choices":[{"delta":{},"finish_reason":"length","index":0}]}"#,
            &mut Silent,
        )
        .unwrap();

        assert_eq!(acc.into_result().stop_reason, StopReason::MaxTokens);
    }
}
