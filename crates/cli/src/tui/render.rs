use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

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
    let busy = if app.state == AppState::Busy {
        " ●"
    } else {
        ""
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
                for line in text.lines() {
                    lines.push(Line::raw(line.to_string()));
                }

                lines.push(Line::default());
            }

            DisplayMessage::ToolUse {
                name,
                input,
                output,
                is_error,
            } => {
                render_tool_block(&mut lines, name, input, output, *is_error);
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
) {
    let border = Style::new().fg(Color::DarkGray);

    // Header
    lines.push(Line::from(vec![
        Span::styled("┌ ", border),
        Span::styled(name, Style::new().fg(Color::Yellow).bold()),
        Span::styled(" ─".to_string() + &"─".repeat(20), border),
    ]));

    // Input
    if let Some(input) = input {
        let display = format_tool_input(input);

        for line in display.lines() {
            lines.push(Line::from(vec![
                Span::styled("│ ", border),
                Span::styled(line.to_string(), Style::new().fg(Color::White)),
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

        const MAX_LINES: usize = 10;
        let output_lines: Vec<&str> = output.lines().collect();
        let total = output_lines.len();

        for line in output_lines.iter().take(MAX_LINES) {
            lines.push(Line::from(vec![
                Span::styled("│ ", border),
                Span::styled((*line).to_string(), style),
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

fn format_tool_input(input: &serde_json::Value) -> String {
    match input.get("command").and_then(|c| c.as_str()) {
        Some(cmd) => cmd.to_string(),
        None => serde_json::to_string_pretty(input).unwrap_or_default(),
    }
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
