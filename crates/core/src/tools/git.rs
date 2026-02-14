use std::path::Path;

use super::{ToolDef, ToolOutput};

pub struct GitTool;

impl ToolDef for GitTool {
    fn name(&self) -> &'static str {
        "Git"
    }

    fn description(&self) -> &'static str {
        "Git operations via libgit2: status, diff, log, show, blame, branch, add, commit, push, reset, checkout. \
         Does not shell out to git — works directly with the repository."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "subcommand": {
                    "type": "string",
                    "enum": [
                        "status", "diff_staged", "diff_unstaged", "diff", "log", "show", "blame", "branch",
                        "add", "commit", "push", "reset", "checkout", "create_branch", "delete_branch", "unstage"
                    ],
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
                },
                "pathspec": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "File patterns for add/unstage (e.g. ['.', 'src/*.rs'])"
                },
                "message": {
                    "type": "string",
                    "description": "Commit message"
                },
                "remote": {
                    "type": "string",
                    "description": "Remote name for push (default: 'origin')"
                },
                "refspec": {
                    "type": "string",
                    "description": "Refspec for push (e.g. 'refs/heads/main:refs/heads/main')"
                },
                "target": {
                    "type": "string",
                    "description": "Target commit/branch for reset or checkout"
                },
                "mode": {
                    "type": "string",
                    "enum": ["soft", "mixed", "hard"],
                    "description": "Reset mode (default: mixed)"
                },
                "branch_name": {
                    "type": "string",
                    "description": "Branch name for create/checkout/delete"
                },
                "start_point": {
                    "type": "string",
                    "description": "Starting point for new branch (default: HEAD)"
                },
                "force": {
                    "type": "boolean",
                    "description": "Force operation (for push, delete_branch, etc.)"
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
            // Read-only operations
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

            // Write operations
            "add" => {
                let pathspec = match input.get("pathspec").and_then(|v| v.as_array()) {
                    Some(arr) => arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>(),
                    None => return ToolOutput::error("add requires 'pathspec' array"),
                };
                exec_add(cwd, &pathspec)
            }
            "unstage" => {
                let pathspec = match input.get("pathspec").and_then(|v| v.as_array()) {
                    Some(arr) => arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>(),
                    None => return ToolOutput::error("unstage requires 'pathspec' array"),
                };
                exec_unstage(cwd, &pathspec)
            }
            "commit" => {
                let message = match input.get("message").and_then(|v| v.as_str()) {
                    Some(m) => m,
                    None => return ToolOutput::error("commit requires 'message' parameter"),
                };
                exec_commit(cwd, message)
            }
            "push" => {
                let remote = input
                    .get("remote")
                    .and_then(|v| v.as_str())
                    .unwrap_or("origin");
                let refspec = match input.get("refspec").and_then(|v| v.as_str()) {
                    Some(r) => r,
                    None => return ToolOutput::error("push requires 'refspec' parameter"),
                };
                let force = input
                    .get("force")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                exec_push(cwd, remote, refspec, force)
            }
            "reset" => {
                let target = match input.get("target").and_then(|v| v.as_str()) {
                    Some(t) => t,
                    None => return ToolOutput::error("reset requires 'target' parameter"),
                };
                let mode_str = input
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("mixed");
                let mode: ccrs_git::ResetMode = match mode_str.parse() {
                    Ok(m) => m,
                    Err(_) => {
                        return ToolOutput::error(
                            "Invalid reset mode (expected: soft, mixed, hard)",
                        );
                    }
                };
                exec_reset(cwd, target, mode)
            }
            "checkout" => {
                let branch_name = match input.get("branch_name").and_then(|v| v.as_str()) {
                    Some(b) => b,
                    None => return ToolOutput::error("checkout requires 'branch_name' parameter"),
                };
                exec_checkout(cwd, branch_name)
            }
            "create_branch" => {
                let branch_name = match input.get("branch_name").and_then(|v| v.as_str()) {
                    Some(b) => b,
                    None => {
                        return ToolOutput::error("create_branch requires 'branch_name' parameter");
                    }
                };
                let start_point = input.get("start_point").and_then(|v| v.as_str());
                exec_create_branch(cwd, branch_name, start_point)
            }
            "delete_branch" => {
                let branch_name = match input.get("branch_name").and_then(|v| v.as_str()) {
                    Some(b) => b,
                    None => {
                        return ToolOutput::error("delete_branch requires 'branch_name' parameter");
                    }
                };
                let force = input
                    .get("force")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                exec_delete_branch(cwd, branch_name, force)
            }

            other => ToolOutput::error(format!(
                "Unknown subcommand: {other}. Expected: status, diff_staged, diff_unstaged, diff, log, show, blame, branch, add, commit, push, reset, checkout, create_branch, delete_branch, unstage"
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

// ---------------------------------------------------------------------------
// Write operations
// ---------------------------------------------------------------------------

fn exec_add(cwd: &Path, pathspec: &[&str]) -> ToolOutput {
    match ccrs_git::add(cwd, pathspec) {
        Ok(_) => {
            let files = pathspec.join(", ");
            ToolOutput::success(format!("Staged: {files}"))
        }
        Err(e) => ToolOutput::error(format!("git add failed: {e}")),
    }
}

fn exec_unstage(cwd: &Path, pathspec: &[&str]) -> ToolOutput {
    match ccrs_git::unstage(cwd, pathspec) {
        Ok(_) => {
            let files = pathspec.join(", ");
            ToolOutput::success(format!("Unstaged: {files}"))
        }
        Err(e) => ToolOutput::error(format!("git unstage failed: {e}")),
    }
}

fn exec_commit(cwd: &Path, message: &str) -> ToolOutput {
    match ccrs_git::commit(cwd, message) {
        Ok(oid) => ToolOutput::success(format!("Created commit {}", &oid[..8])),
        Err(e) => ToolOutput::error(format!("git commit failed: {e}")),
    }
}

fn exec_push(cwd: &Path, remote: &str, refspec: &str, force: bool) -> ToolOutput {
    match ccrs_git::push(cwd, remote, refspec, force) {
        Ok(msg) => ToolOutput::success(msg),
        Err(e) => ToolOutput::error(format!("git push failed: {e}")),
    }
}

fn exec_reset(cwd: &Path, target: &str, mode: ccrs_git::ResetMode) -> ToolOutput {
    match ccrs_git::reset(cwd, target, mode) {
        Ok(_) => ToolOutput::success(format!("Reset to {target}")),
        Err(e) => ToolOutput::error(format!("git reset failed: {e}")),
    }
}

fn exec_checkout(cwd: &Path, branch_name: &str) -> ToolOutput {
    match ccrs_git::checkout(cwd, branch_name) {
        Ok(_) => ToolOutput::success(format!("Switched to branch '{branch_name}'")),
        Err(e) => ToolOutput::error(format!("git checkout failed: {e}")),
    }
}

fn exec_create_branch(cwd: &Path, branch_name: &str, start_point: Option<&str>) -> ToolOutput {
    match ccrs_git::create_branch(cwd, branch_name, start_point) {
        Ok(_) => {
            let from = start_point.unwrap_or("HEAD");
            ToolOutput::success(format!("Created branch '{branch_name}' from {from}"))
        }
        Err(e) => ToolOutput::error(format!("git branch failed: {e}")),
    }
}

fn exec_delete_branch(cwd: &Path, branch_name: &str, force: bool) -> ToolOutput {
    match ccrs_git::delete_branch(cwd, branch_name, force) {
        Ok(_) => ToolOutput::success(format!("Deleted branch '{branch_name}'")),
        Err(e) => ToolOutput::error(format!("git branch -d failed: {e}")),
    }
}
