use std::io::BufRead;
use std::path::Path;

use super::{ToolDef, ToolOutput};

pub struct GrepTool;

impl ToolDef for GrepTool {
    fn name(&self) -> &'static str {
        "Grep"
    }

    fn description(&self) -> &'static str {
        "Search tool for finding patterns in file contents using regular expressions. \
         Supports context lines and multiple output modes."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in (defaults to working directory)"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g. \"*.rs\", \"*.{ts,tsx}\")"
                },
                "output_mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "description": "Output mode (default: files_with_matches)"
                },
                "-i": {
                    "type": "boolean",
                    "description": "Case insensitive search"
                },
                "-n": {
                    "type": "boolean",
                    "description": "Show line numbers (default: true)"
                },
                "-A": {
                    "type": "integer",
                    "description": "Lines to show after each match"
                },
                "-B": {
                    "type": "integer",
                    "description": "Lines to show before each match"
                },
                "-C": {
                    "type": "integer",
                    "description": "Lines to show before and after each match"
                },
                "head_limit": {
                    "type": "integer",
                    "description": "Limit output to first N entries"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, input: &serde_json::Value, cwd: &Path) -> ToolOutput {
        let pattern = match input.get("pattern").and_then(|p| p.as_str()) {
            Some(p) => p,
            None => return ToolOutput::error("Missing required parameter: pattern"),
        };

        let case_insensitive = input.get("-i").and_then(|v| v.as_bool()).unwrap_or(false);

        let regex = match regex::RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build()
        {
            Ok(r) => r,
            Err(e) => return ToolOutput::error(format!("Invalid regex: {e}")),
        };

        let search_path = match input.get("path").and_then(|p| p.as_str()) {
            Some(p) if Path::new(p).is_absolute() => Path::new(p).to_path_buf(),
            Some(p) => cwd.join(p),
            None => cwd.to_path_buf(),
        };

        let glob_filter = input.get("glob").and_then(|g| g.as_str());
        let output_mode = input
            .get("output_mode")
            .and_then(|m| m.as_str())
            .unwrap_or("files_with_matches");

        let head_limit = input
            .get("head_limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let context_after = input
            .get("-A")
            .and_then(|v| v.as_u64())
            .or_else(|| input.get("-C").and_then(|v| v.as_u64()))
            .unwrap_or(0) as usize;

        let context_before = input
            .get("-B")
            .and_then(|v| v.as_u64())
            .or_else(|| input.get("-C").and_then(|v| v.as_u64()))
            .unwrap_or(0) as usize;

        let show_line_numbers = input.get("-n").and_then(|v| v.as_bool()).unwrap_or(true);

        // Collect files to search
        let files = collect_files(&search_path, glob_filter);

        let mut output = String::new();
        let mut entry_count = 0usize;

        for file_path in &files {
            if head_limit.is_some_and(|limit| entry_count >= limit) {
                break;
            }

            let file_content = match std::fs::read(file_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Skip binary files
            if file_content.iter().take(8192).any(|&b| b == 0) {
                continue;
            }

            let lines: Vec<String> = file_content
                .lines()
                .map(|l| l.unwrap_or_default())
                .collect();

            let matches: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, line)| regex.is_match(line))
                .map(|(i, _)| i)
                .collect();

            if matches.is_empty() {
                continue;
            }

            match output_mode {
                "files_with_matches" => {
                    output.push_str(&file_path.display().to_string());
                    output.push('\n');
                    entry_count += 1;
                }
                "count" => {
                    output.push_str(&format!("{}:{}\n", file_path.display(), matches.len()));
                    entry_count += 1;
                }
                _ => {
                    for &match_line in &matches {
                        if head_limit.is_some_and(|limit| entry_count >= limit) {
                            break;
                        }

                        let start = match_line.saturating_sub(context_before);
                        let end = (match_line + context_after + 1).min(lines.len());

                        for (i, line) in lines[start..end].iter().enumerate() {
                            let line_idx = start + i;

                            if show_line_numbers {
                                let marker = if line_idx == match_line { ":" } else { "-" };

                                output.push_str(&format!(
                                    "{}{}{}{marker}",
                                    file_path.display(),
                                    marker,
                                    line_idx + 1,
                                ));
                            } else {
                                output.push_str(&format!("{}:", file_path.display()));
                            }

                            output.push_str(line);
                            output.push('\n');
                        }

                        if context_before > 0 || context_after > 0 {
                            output.push_str("--\n");
                        }

                        entry_count += 1;
                    }
                }
            }
        }

        if output.is_empty() {
            return ToolOutput::success("No matches found.");
        }

        ToolOutput::success(output.trim_end())
    }
}

fn collect_files(path: &Path, glob_filter: Option<&str>) -> Vec<std::path::PathBuf> {
    let glob_matcher = glob_filter.and_then(|g| glob::Pattern::new(g).ok());

    let mut files = Vec::new();

    if path.is_file() {
        files.push(path.to_path_buf());
        return files;
    }

    let walker = ignore::WalkBuilder::new(path)
        .hidden(false)
        .git_ignore(true)
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let entry_path = entry.path();

        if let Some(ref matcher) = glob_matcher {
            let file_name = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            let relative = entry_path.display().to_string();

            if !matcher.matches(file_name) && !matcher.matches(&relative) {
                continue;
            }
        }

        files.push(entry_path.to_path_buf());
    }

    files
}
