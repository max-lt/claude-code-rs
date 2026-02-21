use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use crate::api::{ApiClient, Content, ContentBlock, Message, StopReason, ThinkingConfig, Usage};
use crate::event::EventHandler;
use crate::permission::{AllowAll, PermissionHandler};
use crate::tools::{self, ToolRegistry};

pub struct Session<P: PermissionHandler> {
    client: ApiClient,
    cwd: PathBuf,
    permissions: P,
    messages: Vec<Message>,
    bootstrap_len: usize,
    system_prompt: String,
    tools: ToolRegistry,
}

pub struct SessionBuilder {
    access_token: String,
    is_oauth: bool,
    cwd: Option<PathBuf>,
}

impl SessionBuilder {
    pub fn new(access_token: String, is_oauth: bool) -> Self {
        Self {
            access_token,
            is_oauth,
            cwd: None,
        }
    }

    #[must_use]
    pub fn cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }

    pub fn permissions<P: PermissionHandler>(self, permissions: P) -> Result<Session<P>> {
        let cwd = match self.cwd {
            Some(cwd) => cwd,
            None => std::env::current_dir().context("Failed to determine current directory")?,
        };

        let mut system_prompt =
            "You are Claude Code, Anthropic's official CLI for Claude.".to_string();

        // Load project instructions (CLAUDE.md, .claude/instructions.md)
        for instructions in load_project_instructions(&cwd) {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(&instructions);
        }

        let git_tool_line = if cfg!(feature = "git") {
            "\n             - **Git**: Git operations (status, diff, log, branch, add, commit, push, reset, checkout) via libgit2. Prefer this over `git` CLI."
        } else {
            ""
        };

        let search_tool_line = if cfg!(feature = "search") {
            "\n             - **Search**: Full-text search across the codebase with BM25 ranking."
        } else {
            ""
        };

        let context_prompt = format!(
            "Working directory: {cwd}\n\
             \n\
             You have access to these tools:\n\
             - **Bash**: Execute shell commands. Use for running programs, builds, etc.\n\
             - **Read**: Read a file's contents. Always prefer this over `cat` or `head`.\n\
             - **Write**: Write content to a file. Always prefer this over shell redirects.\n\
             - **Edit**: Perform exact string replacements in files.\n\
             - **Glob**: Find files by glob pattern (e.g. \"**/*.rs\"). Use this instead of `find`.\n\
             - **List**: List directory contents. Use this instead of `ls`.\n\
             - **Fetch**: Make HTTP requests (GET, POST, etc.). Use this instead of curl/wget.\n\
             - **Grep**: Search file contents with regex. Use this instead of `grep`.{git_tool_line}{search_tool_line}\n\
             \n\
             Important:\n\
             - Use Read/Write/Edit instead of Bash for file operations.\n\
             - Use List instead of `ls`, Glob instead of `find`, Grep instead of `grep`.\n\
             - Use Fetch instead of curl/wget for HTTP requests.{git_use_hint}\n\
             - Keep responses concise.\n\
             - When executing commands, use the working directory as the base for relative paths.",
            cwd = cwd.display(),
            git_use_hint = if cfg!(feature = "git") {
                "\n             - Use the Git tool instead of `git` CLI for status, diff, log, and branch operations."
            } else {
                ""
            },
        );

        let bootstrap_messages = vec![
            Message {
                role: "user".to_string(),
                content: Content::text(context_prompt),
            },
            Message {
                role: "assistant".to_string(),
                content: Content::text(
                    "Understood. I'll use the available tools and keep responses concise. How can I help?",
                ),
            },
        ];

        let bootstrap_len = bootstrap_messages.len();

        Ok(Session {
            client: ApiClient::new(self.access_token, self.is_oauth),
            cwd,
            permissions,
            messages: bootstrap_messages,
            bootstrap_len,
            system_prompt,
            tools: tools::default_registry(),
        })
    }

    pub fn build(self) -> Result<Session<AllowAll>> {
        self.permissions(AllowAll)
    }
}

impl<P: PermissionHandler> Session<P> {
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn permissions_mut(&mut self) -> &mut P {
        &mut self.permissions
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn clear(&mut self) {
        self.messages.truncate(self.bootstrap_len);
    }

    pub fn model(&self) -> &str {
        self.client.model()
    }

    pub fn set_model(&mut self, model: String) {
        self.client.set_model(model);
    }

    pub fn thinking(&self) -> &ThinkingConfig {
        self.client.thinking()
    }

    pub fn set_thinking(&mut self, config: ThinkingConfig) {
        self.client.set_thinking(config);
    }

    pub fn set_temperature(&mut self, temp: Option<f32>) {
        self.client.set_temperature(temp);
    }

    pub async fn send_message(
        &mut self,
        input: &str,
        handler: &mut dyn EventHandler,
        cancel: &CancellationToken,
    ) -> Result<Usage> {
        // Save message count so we can roll back the entire turn on error.
        let rollback_len = self.messages.len();

        self.messages.push(Message {
            role: "user".to_string(),
            content: Content::text(input),
        });

        let tool_defs = self.tools.api_definitions();
        let tools_param = if tool_defs.is_empty() {
            None
        } else {
            Some(tool_defs.as_slice())
        };

        let mut total_usage = Usage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };

        loop {
            if cancel.is_cancelled() {
                break;
            }

            let result = self
                .client
                .stream_message(
                    &self.messages,
                    Some(&self.system_prompt),
                    tools_param,
                    handler,
                    cancel,
                )
                .await;

            let stream_result = match result {
                Ok(r) => r,
                Err(e) => {
                    // Roll back all messages added during this turn
                    // (user message + any assistant/tool_result pairs from
                    // previous loop iterations).
                    self.messages.truncate(rollback_len);
                    return Err(e);
                }
            };

            total_usage.input_tokens += stream_result.usage.input_tokens;
            total_usage.output_tokens += stream_result.usage.output_tokens;
            total_usage.cache_creation_input_tokens +=
                stream_result.usage.cache_creation_input_tokens;
            total_usage.cache_read_input_tokens += stream_result.usage.cache_read_input_tokens;

            // Push assistant message with all content blocks
            self.messages.push(Message {
                role: "assistant".to_string(),
                content: Content::blocks(stream_result.content.clone()),
            });

            if stream_result.stop_reason != StopReason::ToolUse {
                break;
            }

            // Execute tool calls and collect results
            let tool_results = self
                .execute_tool_calls(&stream_result.content, handler)
                .await;

            if tool_results.is_empty() {
                break;
            }

            // Push tool results as a user message
            self.messages.push(Message {
                role: "user".to_string(),
                content: Content::blocks(tool_results),
            });
        }

        Ok(total_usage)
    }

    /// Execute tool calls from the assistant response.
    ///
    /// Tools that pass their permission check are executed **concurrently**,
    /// which can significantly reduce latency when the model requests several
    /// independent tools in a single turn (e.g. Glob + Grep + Read).
    async fn execute_tool_calls(
        &mut self,
        content: &[ContentBlock],
        handler: &mut dyn EventHandler,
    ) -> Vec<ContentBlock> {
        // -----------------------------------------------------------------
        // Phase 1 (sequential): permission checks, UI events, preparation
        // -----------------------------------------------------------------

        /// A tool call that passed the permission check and is ready to run.
        struct PreparedCall<'a> {
            id: String,
            name: String,
            input: serde_json::Value,
            tool: &'a dyn tools::ToolDefDyn,
        }

        let mut immediate_results: Vec<ContentBlock> = Vec::new();
        let mut prepared: Vec<PreparedCall<'_>> = Vec::new();

        for block in content {
            let (id, name, input) = match block {
                ContentBlock::ToolUse { id, name, input } => (id, name, input),
                _ => continue,
            };

            handler.on_tool_use_start(name, id, input);

            // Permission check (requires &mut self.permissions)
            let perm_tool = tools::to_permission_tool(name, input);
            let allowed = match &perm_tool {
                Some(tool) => self.permissions.allow(tool),
                None => false,
            };

            if !allowed {
                handler.on_tool_use_end(name);
                immediate_results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: "Permission denied by user.".to_string(),
                    is_error: Some(true),
                });
                continue;
            }

            handler.on_tool_executing(name, input);

            match self.tools.get(name) {
                Some(tool) => {
                    prepared.push(PreparedCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                        tool,
                    });
                }
                None => {
                    let output = tools::ToolOutput::error(format!("Unknown tool: {name}"));
                    handler.on_tool_result(name, &output.content, output.is_error);
                    handler.on_tool_use_end(name);
                    immediate_results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: output.content,
                        is_error: Some(true),
                    });
                }
            }
        }

        // -----------------------------------------------------------------
        // Phase 2 (parallel): execute all approved tools concurrently
        // -----------------------------------------------------------------

        let cwd = &self.cwd;
        let outputs = futures::future::join_all(
            prepared
                .iter()
                .map(|call| call.tool.execute_dyn(&call.input, cwd)),
        )
        .await;

        // -----------------------------------------------------------------
        // Phase 3 (sequential): collect results, emit UI events
        // -----------------------------------------------------------------

        let mut results = immediate_results;
        for (call, output) in prepared.iter().zip(outputs) {
            handler.on_tool_result(&call.name, &output.content, output.is_error);
            handler.on_tool_use_end(&call.name);
            results.push(ContentBlock::ToolResult {
                tool_use_id: call.id.clone(),
                content: output.content,
                is_error: if output.is_error { Some(true) } else { None },
            });
        }

        results
    }
}

/// Load project-level instructions from well-known files.
///
/// Checks (in order):
/// 1. `CLAUDE.md` — project root
/// 2. `.claude/instructions.md` — project-local instructions
///
/// Returns contents of all files that exist and are non-empty.
fn load_project_instructions(cwd: &Path) -> Vec<String> {
    let candidates = [
        cwd.join("CLAUDE.md"),
        cwd.join(".claude").join("instructions.md"),
    ];

    candidates
        .iter()
        .filter_map(|path| {
            let content = fs::read_to_string(path).ok()?;
            let trimmed = content.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect()
}
