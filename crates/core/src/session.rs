use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::api::{ApiClient, Content, ContentBlock, Message, StopReason, Usage};
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

        let system_prompt = "You are Claude Code, Anthropic's official CLI for Claude.".to_string();

        let git_tool_line = if cfg!(feature = "git") {
            "\n             - **Git**: Git operations (status, diff, log, branch) via libgit2. Prefer this over `git` CLI."
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
             - **Glob**: Find files by glob pattern (e.g. \"**/*.rs\").\n\
             - **Grep**: Search file contents with regex.{git_tool_line}{search_tool_line}\n\
             \n\
             Important:\n\
             - Use Read/Write/Edit instead of Bash for file operations.\n\
             - Use Glob/Grep instead of find/grep commands.{git_use_hint}\n\
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

    pub async fn send_message(
        &mut self,
        input: &str,
        handler: &mut dyn EventHandler,
    ) -> Result<Usage> {
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
        };

        loop {
            let result = self
                .client
                .stream_message(
                    &self.messages,
                    Some(&self.system_prompt),
                    tools_param,
                    handler,
                )
                .await;

            let stream_result = match result {
                Ok(r) => r,
                Err(e) => {
                    self.messages.pop(); // rollback
                    return Err(e);
                }
            };

            total_usage.input_tokens += stream_result.usage.input_tokens;
            total_usage.output_tokens += stream_result.usage.output_tokens;

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

    async fn execute_tool_calls(
        &mut self,
        content: &[ContentBlock],
        handler: &mut dyn EventHandler,
    ) -> Vec<ContentBlock> {
        let mut results = Vec::new();

        for block in content {
            let (id, name, input) = match block {
                ContentBlock::ToolUse { id, name, input } => (id, name, input),
                _ => continue,
            };

            handler.on_tool_use_start(name, id);

            // Permission check
            let perm_tool = tools::to_permission_tool(name, input);
            let allowed = match &perm_tool {
                Some(tool) => self.permissions.allow(tool),
                None => false,
            };

            let result = if !allowed {
                ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: "Permission denied by user.".to_string(),
                    is_error: Some(true),
                }
            } else {
                handler.on_tool_executing(name, input);

                let output = match self.tools.get(name) {
                    Some(tool) => tool.execute_dyn(input, &self.cwd).await,
                    None => tools::ToolOutput::error(format!("Unknown tool: {name}")),
                };

                handler.on_tool_result(name, &output.content, output.is_error);

                ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: output.content,
                    is_error: if output.is_error { Some(true) } else { None },
                }
            };

            handler.on_tool_use_end(name);
            results.push(result);
        }

        results
    }
}
