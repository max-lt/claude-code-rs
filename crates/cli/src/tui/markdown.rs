use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Convert markdown text to ratatui Lines with styling.
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut current_line = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut in_code_block = false;
    let mut code_block_lines: Vec<String> = Vec::new();
    let mut list_indent = 0;

    let options = Options::all();
    let parser = Parser::new_ext(text, options);

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { .. } => {
                    style_stack.push(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                Tag::Emphasis => {
                    style_stack.push(Style::default().add_modifier(Modifier::ITALIC));
                }
                Tag::Strong => {
                    style_stack.push(Style::default().add_modifier(Modifier::BOLD));
                }
                Tag::CodeBlock(_) => {
                    in_code_block = true;
                    code_block_lines.clear();
                }
                Tag::Link { .. } => {
                    style_stack.push(
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::UNDERLINED),
                    );
                }
                Tag::List(_) => {
                    list_indent += 2;
                }
                Tag::Item => {
                    if !current_line.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current_line)));
                    }
                    current_line.push(Span::raw("  ".repeat(list_indent / 2)));
                    current_line.push(Span::styled("• ", Style::default().fg(Color::Yellow)));
                }
                _ => {}
            },

            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) | TagEnd::Emphasis | TagEnd::Strong | TagEnd::Link => {
                    style_stack.pop();
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    if !current_line.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current_line)));
                    }

                    // Render code block with background
                    for code_line in &code_block_lines {
                        lines.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(code_line.clone(), Style::default().fg(Color::Green)),
                        ]));
                    }

                    code_block_lines.clear();
                }
                TagEnd::List(_) => {
                    list_indent = list_indent.saturating_sub(2);
                }
                TagEnd::Paragraph => {
                    if !current_line.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current_line)));
                    }
                    lines.push(Line::default()); // blank line after paragraph
                }
                _ => {}
            },

            Event::Text(text) => {
                if in_code_block {
                    code_block_lines.push(text.to_string());
                } else {
                    let current_style = *style_stack.last().unwrap_or(&Style::default());
                    current_line.push(Span::styled(text.to_string(), current_style));
                }
            }

            Event::Code(code) => {
                current_line.push(Span::styled(
                    code.to_string(),
                    Style::default().fg(Color::Green),
                ));
            }

            Event::SoftBreak | Event::HardBreak => {
                if !in_code_block && !current_line.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_line)));
                }
            }

            Event::Rule => {
                if !current_line.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_line)));
                }
                lines.push(Line::styled(
                    "─".repeat(80),
                    Style::default().fg(Color::DarkGray),
                ));
                lines.push(Line::default());
            }

            _ => {}
        }
    }

    // Flush remaining content
    if !current_line.is_empty() {
        lines.push(Line::from(current_line));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_markdown() {
        let md = "# Hello\n\nThis is **bold** and *italic*.";
        let lines = render_markdown(md);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_code_block() {
        let md = "```rust\nfn main() {}\n```";
        let lines = render_markdown(md);
        assert!(!lines.is_empty());
    }
}
