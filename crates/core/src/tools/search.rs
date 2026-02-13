use std::path::Path;
use std::sync::Mutex;

use super::{ToolDef, ToolOutput};

pub struct SearchTool {
    index: Mutex<Option<ccrs_search::SearchIndex>>,
}

impl Default for SearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchTool {
    pub fn new() -> Self {
        Self {
            index: Mutex::new(None),
        }
    }

    fn ensure_index(&self, cwd: &Path) -> Result<(), String> {
        let mut guard = self.index.lock().map_err(|e| e.to_string())?;

        if let Some(index) = guard.as_mut() {
            // Incremental update
            let stats = index.update().map_err(|e| e.to_string())?;

            if stats.has_changes() {
                eprintln!(
                    "Index updated: +{} ~{} -{}",
                    stats.added, stats.modified, stats.removed
                );
            }
        } else {
            // First build
            let (index, stats) = ccrs_search::SearchIndex::open(cwd).map_err(|e| e.to_string())?;

            eprintln!(
                "Index built: {} files, {:.1} KB",
                stats.files,
                stats.bytes as f64 / 1024.0
            );

            *guard = Some(index);
        }

        Ok(())
    }
}

impl ToolDef for SearchTool {
    fn name(&self) -> &'static str {
        "Search"
    }

    fn description(&self) -> &'static str {
        "Semantic + keyword search across the codebase using hybrid BM25/embedding ranking. \
         Builds an in-memory index on first use (with lazy embedding), then updates incrementally. \
         Returns ranked results with optional line-numbered snippets."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query — works with both exact terms and conceptual/semantic queries"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10)"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Number of context lines around matches in snippets (default: 2)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: &serde_json::Value, cwd: &Path) -> ToolOutput {
        let query = match input.get("query").and_then(|q| q.as_str()) {
            Some(q) => q,
            None => return ToolOutput::error("Missing required parameter: query"),
        };

        let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

        let context_lines = input
            .get("context_lines")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as usize;

        if let Err(e) = self.ensure_index(cwd) {
            return ToolOutput::error(format!("Failed to build search index: {e}"));
        }

        let mut guard = match self.index.lock() {
            Ok(g) => g,
            Err(e) => return ToolOutput::error(format!("Index lock error: {e}")),
        };

        let index = match guard.as_mut() {
            Some(i) => i,
            None => return ToolOutput::error("Search index not available"),
        };

        let hits = match index.search(query, limit, context_lines) {
            Ok(h) => h,
            Err(e) => return ToolOutput::error(format!("Search failed: {e}")),
        };

        if hits.is_empty() {
            return ToolOutput::success("No results found.");
        }

        let mut output = String::new();

        for (i, hit) in hits.iter().enumerate() {
            output.push_str(&format!(
                "{}. {} (score: {:.4})\n",
                i + 1,
                hit.path,
                hit.score
            ));

            for snippet in &hit.snippets {
                for (j, line) in snippet.lines.iter().enumerate() {
                    let line_num = snippet.line_number + j;
                    output.push_str(&format!("  {line_num:>4} | {line}\n"));
                }

                output.push('\n');
            }
        }

        ToolOutput::success(output.trim_end())
    }
}
