use std::path::Path;

use super::{ToolDef, ToolOutput};

pub struct GitTool;

impl ToolDef for GitTool {
    fn name(&self) -> &'static str {
        "Git"
    }

    fn description(&self) -> &'static str {
        "Git operations via libgit2: status, diff, log, show, blame, branch. \
         Does not shell out to git — works directly with the repository."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "subcommand": {
                    "type": "string",
                    "enum": ["status", "diff_staged", "diff_unstaged", "diff", "log", "show", "blame", "branch"],
                    "description": "The git operation to perform"
                },
                "from": {
                    "type": "string",
                    "description": "Start revision for diff (e.g. 'main', 'HEAD~3', a commit hash)"
                },
                "to": {
                    "type": "string",
                    "description": "End revision for diff (default: HEAD)"
                },
                "rev": {
                    "type": "string",
                    "description": "Revision for show (default: HEAD)"
                },
                "file_path": {
                    "type": "string",
                    "description": "File path (relative to repo root) for blame"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Start line for blame range (1-based, optional)"
                },
                "end_line": {
                    "type": "integer",
                    "description": "End line for blame range (1-based, optional)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max entries for log (default: 20)"
                },
                "include_remote": {
                    "type": "boolean",
                    "description": "Include remote branches in branch listing (default: false)"
                }
            },
            "required": ["subcommand"]
        })
    }

    async fn execute(&self, input: &serde_json::Value, cwd: &Path) -> ToolOutput {
        let subcommand = match input.get("subcommand").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolOutput::error("Missing required parameter: subcommand"),
        };

        match subcommand {
            "status" => exec_status(cwd),
            "diff_staged" => exec_diff_staged(cwd),
            "diff_unstaged" => exec_diff_unstaged(cwd),
            "diff" => {
                let from = match input.get("from").and_then(|v| v.as_str()) {
                    Some(f) => f,
                    None => return ToolOutput::error("diff requires 'from' parameter"),
                };
                let to = input.get("to").and_then(|v| v.as_str()).unwrap_or("HEAD");
                exec_diff_range(cwd, from, to)
            }
            "log" => {
                let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                exec_log(cwd, limit)
            }
            "show" => {
                let rev = input.get("rev").and_then(|v| v.as_str()).unwrap_or("HEAD");
                exec_show(cwd, rev)
            }
            "blame" => {
                let file_path = match input.get("file_path").and_then(|v| v.as_str()) {
                    Some(f) => f,
                    None => return ToolOutput::error("blame requires 'file_path' parameter"),
                };
                let start = input
                    .get("start_line")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let end = input
                    .get("end_line")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                exec_blame(cwd, file_path, start, end)
            }
            "branch" => {
                let include_remote = input
                    .get("include_remote")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                exec_branch(cwd, include_remote)
            }
            other => ToolOutput::error(format!(
                "Unknown subcommand: {other}. Expected: status, diff_staged, diff_unstaged, diff, log, show, blame, branch"
            )),
        }
    }
}

fn exec_status(cwd: &Path) -> ToolOutput {
    match ccrs_git::status(cwd) {
        Ok(entries) => {
            if entries.is_empty() {
                return ToolOutput::success("Working tree clean.");
            }
            let out: String = entries.iter().map(|e| format!("{e}\n")).collect();
            ToolOutput::success(out.trim_end())
        }
        Err(e) => ToolOutput::error(format!("git status failed: {e}")),
    }
}

fn exec_diff_staged(cwd: &Path) -> ToolOutput {
    match ccrs_git::diff_staged(cwd) {
        Ok((entries, stat)) => format_diff(entries, stat),
        Err(e) => ToolOutput::error(format!("git diff --cached failed: {e}")),
    }
}

fn exec_diff_unstaged(cwd: &Path) -> ToolOutput {
    match ccrs_git::diff_unstaged(cwd) {
        Ok((entries, stat)) => format_diff(entries, stat),
        Err(e) => ToolOutput::error(format!("git diff failed: {e}")),
    }
}

fn exec_diff_range(cwd: &Path, from: &str, to: &str) -> ToolOutput {
    match ccrs_git::diff_range(cwd, from, to) {
        Ok((entries, stat)) => format_diff(entries, stat),
        Err(e) => ToolOutput::error(format!("git diff {from}..{to} failed: {e}")),
    }
}

fn format_diff(entries: Vec<ccrs_git::DiffEntry>, stat: ccrs_git::DiffStat) -> ToolOutput {
    if entries.is_empty() {
        return ToolOutput::success("No changes.");
    }

    let mut out = String::new();

    for entry in &entries {
        out.push_str(&entry.patch);
        if !entry.patch.ends_with('\n') {
            out.push('\n');
        }
    }

    out.push_str(&format!(
        "\n{} file(s) changed, {} insertion(s), {} deletion(s)",
        stat.files_changed, stat.insertions, stat.deletions
    ));

    ToolOutput::success(out)
}

fn exec_log(cwd: &Path, limit: usize) -> ToolOutput {
    match ccrs_git::git_log(cwd, limit) {
        Ok(entries) => {
            if entries.is_empty() {
                return ToolOutput::success("No commits yet.");
            }
            let out: String = entries
                .iter()
                .map(|e| {
                    format!(
                        "{} {} ({}, {})\n",
                        e.short_hash, e.message, e.author, e.date
                    )
                })
                .collect();
            ToolOutput::success(out.trim_end())
        }
        Err(e) => ToolOutput::error(format!("git log failed: {e}")),
    }
}

fn exec_show(cwd: &Path, rev: &str) -> ToolOutput {
    match ccrs_git::show(cwd, rev) {
        Ok(detail) => {
            let mut out = format!(
                "commit {}\nAuthor: {} <{}>\nDate:   {}\n\n    {}\n\n",
                detail.hash,
                detail.author,
                detail.email,
                detail.date,
                detail.message.lines().collect::<Vec<_>>().join("\n    "),
            );

            for entry in &detail.diff_entries {
                out.push_str(&entry.patch);
                if !entry.patch.ends_with('\n') {
                    out.push('\n');
                }
            }

            out.push_str(&format!(
                "\n{} file(s) changed, {} insertion(s), {} deletion(s)",
                detail.stat.files_changed, detail.stat.insertions, detail.stat.deletions,
            ));

            ToolOutput::success(out)
        }
        Err(e) => ToolOutput::error(format!("git show {rev} failed: {e}")),
    }
}

fn exec_blame(cwd: &Path, file_path: &str, start: Option<usize>, end: Option<usize>) -> ToolOutput {
    let result = match (start, end) {
        (Some(s), Some(e)) => ccrs_git::blame_range(cwd, file_path, s, e),
        _ => ccrs_git::blame(cwd, file_path),
    };

    match result {
        Ok(lines) => {
            if lines.is_empty() {
                return ToolOutput::success("No blame data.");
            }
            let out: String = lines
                .iter()
                .map(|l| {
                    format!(
                        "{} ({:<12} {}) {:>4} | {}\n",
                        l.short_hash, l.author, l.date, l.line_number, l.content
                    )
                })
                .collect();
            ToolOutput::success(out.trim_end())
        }
        Err(e) => ToolOutput::error(format!("git blame {file_path} failed: {e}")),
    }
}

fn exec_branch(cwd: &Path, include_remote: bool) -> ToolOutput {
    let current = ccrs_git::current_branch(cwd)
        .ok()
        .flatten()
        .unwrap_or_default();

    match ccrs_git::list_branches(cwd, include_remote) {
        Ok(branches) => {
            if branches.is_empty() {
                return ToolOutput::success("No branches.");
            }
            let mut out = format!("Current branch: {current}\n\n");
            for b in &branches {
                let marker = if b.is_head { "* " } else { "  " };
                let remote = if b.is_remote { " (remote)" } else { "" };
                out.push_str(&format!("{marker}{}{remote}\n", b.name));
            }
            ToolOutput::success(out.trim_end())
        }
        Err(e) => ToolOutput::error(format!("git branch failed: {e}")),
    }
}
