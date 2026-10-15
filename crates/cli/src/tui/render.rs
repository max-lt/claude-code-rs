use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::markdown::render_markdown;
use super::{App, AppState, DisplayMessage, LmStatus, PickerEntry};
use crate::commands;
use claude_code_core::provider::{LM_STUDIO_PREFIX, wire_model};

/// Render the entire UI.
pub fn render(app: &mut App, frame: &mut Frame) {
    let area = frame.area();

    let has_perm = app.pending_perm.is_some();
    let suggestions = slash_suggestions(&app.input);
    let suggestion_height = if app.model_picker.is_some() {
        (app.picker_entries().len() as u16) + 3 // border + lm-studio status + hint
    } else if suggestions.is_empty() {
        0u16
    } else {
        (suggestions.len() as u16) + 1 // +1 for the top border
    };

    let queue_height = queue_panel_height(app);

    let chunks = Layout::vertical([
        Constraint::Length(1),                                  // status bar
        Constraint::Min(1),                                     // messages
        Constraint::Length(if has_perm { 2 } else { 0 }),       // permission prompt
        Constraint::Length(queue_height),                       // queued messages
        Constraint::Length(suggestion_height),                  // command suggestions
        Constraint::Length(3),                                  // input area
    ])
    .split(area);

    render_status_bar(app, frame, chunks[0]);
    render_messages(app, frame, chunks[1]);

    if has_perm {
        render_permission(app, frame, chunks[2]);
    }

    if queue_height > 0 {
        render_queue(app, frame, chunks[3]);
    }

    if let Some(selected) = app.model_picker {
        render_model_picker(app, selected, frame, chunks[4]);
    } else if !suggestions.is_empty() {
        render_suggestions(&suggestions, frame, chunks[4]);
    }

    render_input(app, frame, chunks[5]);
}

/// Most queued messages shown at once; older ones collapse into a summary line.
const MAX_QUEUE_ROWS: usize = 5;

/// Rows taken by the queue panel: one per shown message, plus a summary line
/// when some are hidden. Zero when there is nothing to show.
fn queue_panel_height(app: &App) -> u16 {
    if app.queue.is_empty() {
        return 0;
    }
    let shown = app.queue.len().min(MAX_QUEUE_ROWS);
    let summary = usize::from(app.queue.len() > shown);
    (shown + summary) as u16
}

/// Cut `text` to `max` characters, ending with an ellipsis when it was longer.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// The messages waiting for the current turn to end, oldest first. When there
/// are more than fit, the newest are shown (those are the ones Backspace pops)
/// under a summary of how many are hidden.
fn render_queue(app: &App, frame: &mut Frame, area: Rect) {
    let hidden = app.queue.len().saturating_sub(MAX_QUEUE_ROWS);
    let dim = Style::new().fg(Color::DarkGray);
    let mut lines: Vec<Line> = Vec::new();

    if hidden > 0 {
        lines.push(Line::from(Span::styled(
            format!("  … {hidden} earlier queued"),
            dim,
        )));
    }

    // Marker (2) + indent: keep the text inside the panel width.
    let width = (area.width as usize).saturating_sub(4);

    for text in app.queue.iter().skip(hidden) {
        lines.push(Line::from(vec![
            Span::styled("  ❯ ", dim),
            Span::styled(
                truncate_chars(text, width),
                Style::new().fg(Color::Gray).italic(),
            ),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_status_bar(app: &App, frame: &mut Frame, area: Rect) {
    let mut tokens = format!(
        "{}↑ {}↓",
        format_tokens(app.usage.prompt_tokens()),
        format_tokens(app.usage.output_tokens),
    );

    // Share of prompt tokens served from the cache. Only Anthropic reports it.
    if app.usage.cache_read_input_tokens > 0 {
        let pct = app.usage.cache_read_input_tokens * 100 / app.usage.prompt_tokens().max(1);
        tokens.push_str(&format!(" ({pct}% cached)"));
    }

    let ctx_pct = (app.context_used.saturating_mul(100) / app.context_window.max(1)).min(100);

    let ctx_style = if ctx_pct >= 95 {
        Style::new().fg(Color::Red).bold()
    } else if ctx_pct >= 80 {
        Style::new().fg(Color::Yellow).bold()
    } else {
        Style::new()
    };

    let ctx = format!("ctx {}% ({})", ctx_pct, format_tokens(app.context_window));

    let mut bar = vec![
        Span::styled(" claude-code-rs", Style::new().bold()),
        Span::raw(" │ "),
        Span::raw(display_model(&app.model)),
        Span::raw(" │ "),
        Span::raw(tokens),
        Span::raw(" │ "),
        Span::styled(ctx, ctx_style),
    ];

    if let Some(bytes) = app.memory {
        bar.push(Span::raw(" │ "));
        bar.push(Span::raw(format!("mem {}", format_bytes(bytes))));
    }

    let bar = Line::from(bar);

    let widget = Paragraph::new(bar).style(Style::new().bg(Color::DarkGray).fg(Color::White));
    frame.render_widget(widget, area);
}

fn render_messages(app: &mut App, frame: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    for msg in &app.messages {
        match msg {
            DisplayMessage::User(text) => {
                lines.push(Line::from(vec![
                    Span::styled("> ", Style::new().fg(Color::Cyan).bold()),
                    Span::raw(text.as_str()),
                ]));
                lines.push(Line::default());
            }

            DisplayMessage::AssistantText(text) => {
                let markdown_lines = render_markdown(text);
                lines.extend(markdown_lines);
            }

            DisplayMessage::ToolUse {
                name,
                input,
                output,
                is_error,
            } => {
                render_tool_block(&mut lines, name, input, output, *is_error, &app.cwd);
            }

            DisplayMessage::Thinking(text) => {
                for line in text.lines() {
                    lines.push(Line::styled(
                        line.to_string(),
                        Style::new().fg(Color::DarkGray).italic(),
                    ));
                }
                lines.push(Line::default());
            }

            DisplayMessage::Error(text) => {
                lines.push(Line::styled(
                    format!("Error: {text}"),
                    Style::new().fg(Color::Red),
                ));
                lines.push(Line::default());
            }

            DisplayMessage::Info(text) => {
                for line in text.lines() {
                    lines.push(Line::styled(
                        line.to_string(),
                        Style::new().fg(Color::DarkGray),
                    ));
                }

                lines.push(Line::default());
            }
        }
    }

    let content_height = wrapped_line_count(&lines, area.width);

    // Pad with empty lines so content is bottom-aligned
    if content_height < area.height {
        let padding = area.height - content_height;
        let mut padded = vec![Line::default(); padding as usize];
        padded.append(&mut lines);
        lines = padded;
    }

    let total_height = wrapped_line_count(&lines, area.width);
    let max_scroll = total_height.saturating_sub(area.height);

    // Store max_scroll for scroll event handling
    app.max_scroll = max_scroll;

    let scroll = if app.auto_scroll {
        max_scroll
    } else {
        app.scroll.min(max_scroll)
    };

    let paragraph = Paragraph::new(Text::from(lines))
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

fn render_permission(app: &App, frame: &mut Frame, area: Rect) {
    if let Some(perm) = &app.pending_perm {
        let line = Line::from(vec![
            Span::styled(
                " ? ",
                Style::new().fg(Color::Black).bg(Color::Yellow).bold(),
            ),
            Span::raw(" "),
            Span::raw(&perm.description),
            Span::styled("  [Y/n]", Style::new().fg(Color::DarkGray)),
        ]);

        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::new().fg(Color::Yellow));

        let widget = Paragraph::new(line).block(block);
        frame.render_widget(widget, area);
    }
}

fn render_tool_block<'a>(
    lines: &mut Vec<Line<'a>>,
    name: &'a str,
    input: &Option<serde_json::Value>,
    output: &Option<String>,
    is_error: bool,
    cwd: &Path,
) {
    let border = Style::new().fg(Color::DarkGray);

    // Format header + input based on tool type
    let (header, display) = match input {
        Some(inp) => format_tool_display(name, inp, cwd),
        None => (name.to_string(), None),
    };

    // Header
    lines.push(Line::from(vec![
        Span::styled("┌ ", border),
        Span::styled(header, Style::new().fg(Color::Yellow).bold()),
        Span::styled(" ─".to_string() + &"─".repeat(20), border),
    ]));

    // Input display
    if let Some(display) = &display {
        for line in display.lines() {
            let style = if line.starts_with("- ") {
                Style::new().fg(Color::Red)
            } else if line.starts_with("+ ") {
                Style::new().fg(Color::Green)
            } else {
                Style::new().fg(Color::White)
            };

            lines.push(Line::from(vec![
                Span::styled("│ ", border),
                Span::styled(line.to_string(), style),
            ]));
        }
    }

    // Output
    if let Some(output) = output {
        let style = if is_error {
            Style::new().fg(Color::Red)
        } else {
            Style::new().fg(Color::DarkGray)
        };

        let cwd_prefix = format!("{}/", cwd.display());

        const MAX_LINES: usize = 10;
        let output_lines: Vec<&str> = output.lines().collect();
        let total = output_lines.len();

        for line in output_lines.iter().take(MAX_LINES) {
            let display_line = line.strip_prefix(&cwd_prefix).unwrap_or(line);

            lines.push(Line::from(vec![
                Span::styled("│ ", border),
                Span::styled(display_line.to_string(), style),
            ]));
        }

        if total > MAX_LINES {
            lines.push(Line::from(vec![
                Span::styled("│ ", border),
                Span::styled(
                    format!("... ({total} lines total)"),
                    Style::new().fg(Color::DarkGray).italic(),
                ),
            ]));
        }
    }

    lines.push(Line::styled("└─", border));
    lines.push(Line::default());
}

// ---------------------------------------------------------------------------
// Slash command suggestions
// ---------------------------------------------------------------------------

/// Returns matching command suggestions if the input starts with `/`.
fn slash_suggestions(input: &str) -> Vec<(&'static str, &'static [&'static str], &'static str)> {
    let trimmed = input.trim();

    // Only show suggestions when input starts with `/` and is a single token (no space yet)
    if !trimmed.starts_with('/') || trimmed.contains(' ') {
        return Vec::new();
    }

    commands::matching_commands(trimmed)
        .into_iter()
        .map(|cmd| (cmd.name, cmd.aliases, cmd.description))
        .collect()
}

fn render_suggestions(
    suggestions: &[(&str, &[&str], &str)],
    frame: &mut Frame,
    area: Rect,
) {
    let mut lines: Vec<Line> = Vec::new();

    for (name, aliases, desc) in suggestions {
        let mut spans = vec![
            Span::styled("  ", Style::default()),
            Span::styled(*name, Style::new().fg(Color::Cyan).bold()),
        ];

        if !aliases.is_empty() {
            let alias_str = aliases.join(" ");
            spans.push(Span::styled(
                format!(" {alias_str}"),
                Style::new().fg(Color::DarkGray),
            ));
        }

        spans.push(Span::styled(
            format!("  — {desc}"),
            Style::new().fg(Color::Gray),
        ));

        lines.push(Line::from(spans));
    }

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(Color::DarkGray));

    let widget = Paragraph::new(lines).block(block);
    frame.render_widget(widget, area);
}

// ---------------------------------------------------------------------------
// Model picker
// ---------------------------------------------------------------------------

/// The model name as the user knows it: without the internal LM Studio marker.
fn display_model(model: &str) -> String {
    match model.strip_prefix(LM_STUDIO_PREFIX) {
        Some(_) => format!("[lm-studio] {}", wire_model(model)),
        None => model.to_string(),
    }
}

fn render_model_picker(app: &App, selected: usize, frame: &mut Frame, area: Rect) {
    let entries: Vec<PickerEntry> = app.picker_entries();
    let selected = selected.min(entries.len().saturating_sub(1));
    let mut lines: Vec<Line> = Vec::new();

    for (i, entry) in entries.iter().enumerate() {
        let is_selected = i == selected;

        let pointer = if is_selected { "❯ " } else { "  " };
        let name_style = if is_selected {
            Style::new().fg(Color::Cyan).bold()
        } else {
            Style::new().fg(Color::White)
        };

        let mut spans = vec![
            Span::styled(pointer, Style::new().fg(Color::Cyan).bold()),
            Span::styled(entry.label.clone(), name_style),
        ];

        if !entry.detail.is_empty() {
            spans.push(Span::styled(
                format!("  {}", entry.detail),
                Style::new().fg(Color::DarkGray),
            ));
        }

        if entry.is_default {
            spans.push(Span::styled(" [default]", Style::new().fg(Color::DarkGray)));
        }
        if entry.id == app.model {
            spans.push(Span::styled(" ✓", Style::new().fg(Color::Green).bold()));
        }

        lines.push(Line::from(spans));
    }

    let status = match &app.lm_status {
        LmStatus::Unknown => None,
        LmStatus::Checking => Some("  lm-studio: checking…".to_string()),
        LmStatus::Up if app.lm_models.is_empty() => {
            Some("  lm-studio: up, but no chat model is served".to_string())
        }
        LmStatus::Up => None,
        LmStatus::Down(_) => Some("  lm-studio: not reachable (is the server on :1234?)".to_string()),
    };

    if let Some(status) = status {
        lines.push(Line::styled(status, Style::new().fg(Color::DarkGray)));
    }

    lines.push(Line::styled(
        "  ↑/↓ navigate · Enter select · Esc cancel",
        Style::new().fg(Color::DarkGray).italic(),
    ));

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(Color::Cyan))
        .title(" Select model ");

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

// ---------------------------------------------------------------------------
// Tool display formatting
// ---------------------------------------------------------------------------

/// Returns (header, optional body) for the tool block.
fn format_tool_display(
    name: &str,
    input: &serde_json::Value,
    cwd: &Path,
) -> (String, Option<String>) {
    match name {
        "Bash" => {
            let cmd = str_field(input, "command");
            (format!("Bash({cmd})"), None)
        }

        "Read" => {
            let path = relative_path(str_field(input, "file_path"), cwd);
            (format!("Read {path}"), None)
        }

        "Write" => {
            let path = relative_path(str_field(input, "file_path"), cwd);
            let content = str_field(input, "content");
            let line_count = content.lines().count();
            (format!("Write {path} ({line_count} lines)"), None)
        }

        "Edit" => {
            let path = relative_path(str_field(input, "file_path"), cwd);
            let old = str_field(input, "old_string");
            let new = str_field(input, "new_string");
            let body = format_edit_diff(old, new);
            (format!("Edit {path}"), Some(body))
        }

        "Glob" => {
            let pattern = str_field(input, "pattern");
            let path = input.get("path").and_then(|v| v.as_str());

            let header = match path {
                Some(p) => format!("Glob {pattern} in {}", relative_path(p, cwd)),
                None => format!("Glob {pattern}"),
            };

            (header, None)
        }

        "Grep" => {
            let pattern = str_field(input, "pattern");
            let path = input.get("path").and_then(|v| v.as_str());
            let glob = input.get("glob").and_then(|v| v.as_str());

            let mut header = format!("Grep {pattern}");

            if let Some(g) = glob {
                header.push_str(&format!(" --glob {g}"));
            }

            if let Some(p) = path {
                header.push_str(&format!(" in {}", relative_path(p, cwd)));
            }

            (header, None)
        }

        "List" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| relative_path(p, cwd));

            let header = match path {
                Some(p) => format!("List {p}"),
                None => "List .".to_string(),
            };

            (header, None)
        }

        "Fetch" => {
            let url = str_field(input, "url");
            let method = input
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET");

            (format!("Fetch {method} {url}"), None)
        }

        "Git" => {
            let sub = str_field(input, "subcommand");

            let args = match sub {
                "add" | "unstage" => {
                    if let Some(arr) = input.get("pathspec").and_then(|v| v.as_array()) {
                        let paths: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
                        Some(paths.join(" "))
                    } else {
                        None
                    }
                }

                "commit" => input.get("message").and_then(|v| v.as_str()).map(|s| {
                    // Truncate long messages
                    if s.len() > 60 {
                        format!("\"{}...\"", &s[..60])
                    } else {
                        format!("\"{s}\"")
                    }
                }),

                "push" => {
                    let remote = str_field(input, "remote");
                    let refspec = str_field(input, "refspec");
                    let force = input
                        .get("force")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let force_flag = if force { " --force" } else { "" };
                    Some(format!("{remote} {refspec}{force_flag}"))
                }

                "reset" => {
                    let target = str_field(input, "target");
                    let mode = str_field(input, "mode");
                    Some(format!("{target} --{mode}"))
                }

                "checkout" => input
                    .get("branch_name")
                    .and_then(|v| v.as_str())
                    .map(String::from),

                "create_branch" => {
                    let name = str_field(input, "name");
                    let start_point = input.get("start_point").and_then(|v| v.as_str());
                    match start_point {
                        Some(sp) => Some(format!("{name} (from {sp})")),
                        None => Some(name.to_string()),
                    }
                }

                "delete_branch" => {
                    let name = str_field(input, "name");
                    let force = input
                        .get("force")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let force_flag = if force { " --force" } else { "" };
                    Some(format!("{name}{force_flag}"))
                }

                _ => None,
            };

            let header = match args {
                Some(a) => format!("Git {sub} {a}"),
                None => format!("Git {sub}"),
            };

            (header, None)
        }

        "Search" => {
            let query = str_field(input, "query");
            (format!("Search \"{query}\""), None)
        }

        _ => {
            let body = serde_json::to_string_pretty(input).unwrap_or_default();
            (name.to_string(), Some(body))
        }
    }
}

/// Format an Edit diff: lines prefixed with - and +.
fn format_edit_diff(old: &str, new: &str) -> String {
    let mut out = String::new();

    for line in old.lines() {
        out.push_str(&format!("- {line}\n"));
    }

    for line in new.lines() {
        out.push_str(&format!("+ {line}\n"));
    }

    // Remove trailing newline
    if out.ends_with('\n') {
        out.pop();
    }

    out
}

/// Make a path relative to cwd if it's inside it, otherwise return as-is.
fn relative_path(path: &str, cwd: &Path) -> String {
    let p = Path::new(path);

    match p.strip_prefix(cwd) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.to_string(),
    }
}

/// Extract a string field from JSON input, with empty fallback.
fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
    input.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn render_input(app: &App, frame: &mut Frame, area: Rect) {
    const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    // Queued messages are listed above the input (see `render_queue`), so the
    // input line itself only carries the prompt and what is being typed.
    let prefix = if app.state == AppState::Busy {
        format!("{} ", SPINNER[app.spinner_frame % SPINNER.len()])
    } else {
        "> ".to_string()
    };

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(Color::DarkGray));

    let input_widget = Paragraph::new(format!("{prefix}{}", app.input)).block(block);
    frame.render_widget(input_widget, area);

    // The live input starts right after the prefix.
    let prefix_width = Line::from(prefix).width() as u16;
    let cursor_x = area.x + prefix_width + app.cursor as u16;
    frame.set_cursor_position((cursor_x, area.y + 1));
}

fn format_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Human-readable byte size: `512 KB`, `12.3 MB`, `1.2 GB`.
fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;

    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.0} KB", b / KB)
    }
}

/// Estimate total visual lines after wrapping.
fn wrapped_line_count(lines: &[Line], width: u16) -> u16 {
    let w = width.max(1) as usize;

    lines
        .iter()
        .map(|line| {
            let lw = line.width();

            if lw == 0 { 1u16 } else { lw.div_ceil(w) as u16 }
        })
        .sum()
}
