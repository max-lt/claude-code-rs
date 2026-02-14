use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::markdown::render_markdown;
use super::{App, AppState, DisplayMessage};

/// Render the entire UI.
pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();

    let chunks = Layout::vertical([
        Constraint::Length(1), // status bar
        Constraint::Min(1),    // messages
        Constraint::Length(3), // input area
    ])
    .split(area);

    render_status_bar(app, frame, chunks[0]);
    render_messages(app, frame, chunks[1]);
    render_input(app, frame, chunks[2]);
}

fn render_status_bar(app: &App, frame: &mut Frame, area: Rect) {
    // Spinner frames (unicode braille patterns for smooth animation)
    const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    
    let busy = if app.state == AppState::Busy {
        format!(" {}", SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()])
    } else {
        String::new()
    };

    let tokens = format!(
        "{}↑ {}↓",
        format_tokens(app.usage.input_tokens),
        format_tokens(app.usage.output_tokens),
    );

    let bar = Line::from(vec![
        Span::styled(" claude-code-rs", Style::new().bold()),
        Span::raw(" │ "),
        Span::raw(&app.model),
        Span::raw(" │ "),
        Span::raw(tokens),
        Span::styled(busy, Style::new().fg(Color::Green)),
    ]);

    let widget = Paragraph::new(bar).style(Style::new().bg(Color::DarkGray).fg(Color::White));
    frame.render_widget(widget, area);
}

fn render_messages(app: &App, frame: &mut Frame, area: Rect) {
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

    // Permission prompt inline
    if let Some(perm) = &app.pending_perm {
        lines.push(Line::from(vec![
            Span::styled("? ", Style::new().fg(Color::Yellow).bold()),
            Span::raw(&perm.description),
            Span::styled(" [Y/n] ", Style::new().fg(Color::DarkGray)),
        ]));
    }

    let content_height = wrapped_line_count(&lines, area.width);
    let max_scroll = content_height.saturating_sub(area.height);

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
            (format!("Git {sub}"), None)
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
    let display_text = format!("> {}", app.input);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(Color::DarkGray));

    let input_widget = Paragraph::new(display_text).block(block);
    frame.render_widget(input_widget, area);

    // Position cursor: area.x + 2 (">" + space) + cursor offset, area.y + 1 (border)
    let cursor_x = area.x + 2 + app.cursor as u16;
    let cursor_y = area.y + 1;
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn format_tokens(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
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
