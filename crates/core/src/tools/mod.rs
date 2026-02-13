pub mod bash;
pub mod file_read;
pub mod file_write;

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use crate::permission;

// ---------------------------------------------------------------------------
// Tool output
// ---------------------------------------------------------------------------

pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

// ---------------------------------------------------------------------------
// ToolDef — the user-facing trait (uses async fn directly)
// ---------------------------------------------------------------------------

pub trait ToolDef: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> serde_json::Value;
    fn execute(
        &self,
        input: &serde_json::Value,
        cwd: &Path,
    ) -> impl Future<Output = ToolOutput> + Send;
}

// ---------------------------------------------------------------------------
// ToolDefDyn — object-safe wrapper for dyn dispatch
// ---------------------------------------------------------------------------

pub trait ToolDefDyn: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn input_schema(&self) -> serde_json::Value;
    fn execute_dyn<'a>(
        &'a self,
        input: &'a serde_json::Value,
        cwd: &'a Path,
    ) -> Pin<Box<dyn Future<Output = ToolOutput> + Send + 'a>>;
}

impl<T: ToolDef> ToolDefDyn for T {
    fn name(&self) -> &'static str {
        ToolDef::name(self)
    }

    fn description(&self) -> &'static str {
        ToolDef::description(self)
    }

    fn input_schema(&self) -> serde_json::Value {
        ToolDef::input_schema(self)
    }

    fn execute_dyn<'a>(
        &'a self,
        input: &'a serde_json::Value,
        cwd: &'a Path,
    ) -> Pin<Box<dyn Future<Output = ToolOutput> + Send + 'a>> {
        Box::pin(ToolDef::execute(self, input, cwd))
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

pub struct ToolRegistry {
    tools: Vec<Box<dyn ToolDefDyn>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: impl ToolDef + 'static) {
        self.tools.push(Box::new(tool));
    }

    /// Return tool definitions formatted for the Claude API `tools` parameter.
    pub fn api_definitions(&self) -> Vec<serde_json::Value> {
        self.tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "input_schema": t.input_schema(),
                })
            })
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<&dyn ToolDefDyn> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }
}

/// Create a registry with the default set of tools.
pub fn default_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(bash::BashTool);
    r.register(file_read::FileReadTool);
    r.register(file_write::FileWriteTool);
    r
}

// ---------------------------------------------------------------------------
// Permission mapping
// ---------------------------------------------------------------------------

/// Map an API tool call to the core permission system.
pub fn to_permission_tool<'a>(
    name: &str,
    input: &'a serde_json::Value,
) -> Option<permission::Tool<'a>> {
    match name {
        "bash" => {
            let command = input.get("command").and_then(|c| c.as_str()).unwrap_or("");
            Some(permission::Tool::Bash { command })
        }
        "file_read" => {
            let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("");
            Some(permission::Tool::FileRead {
                path: Path::new(path),
            })
        }
        "file_write" => {
            let path = input.get("path").and_then(|p| p.as_str()).unwrap_or("");
            Some(permission::Tool::FileWrite {
                path: Path::new(path),
            })
        }
        _ => None,
    }
}
