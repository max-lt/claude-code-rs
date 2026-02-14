use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Convert markdown text to ratatui Lines with styling.
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut in_code_block = false;
    let mut code_block_lines: Vec<String> = Vec::new();
    let mut list_depth: usize = 0;

    let options = Options::all();
    let parser = Parser::new_ext(text, options);

    for event in parser {
        match event {
            // ----- Start tags -----
            Event::Start(tag) => match tag {
                Tag::Heading { .. } => {
                    flush_line(&mut lines, &mut current_spans);
                    style_stack.push(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    );
                }

                Tag::Emphasis => {
                    let base = current_style(&style_stack);
                    style_stack.push(base.add_modifier(Modifier::ITALIC));
                }

                Tag::Strong => {
                    let base = current_style(&style_stack);
                    style_stack.push(base.add_modifier(Modifier::BOLD));
                }

                Tag::CodeBlock(_) => {
                    flush_line(&mut lines, &mut current_spans);
                    in_code_block = true;
                    code_block_lines.clear();
                }

                Tag::Link { .. } => {
                    let base = current_style(&style_stack);
                    style_stack.push(base.fg(Color::Blue).add_modifier(Modifier::UNDERLINED));
                }

                Tag::List(_) => {
                    list_depth += 1;
                }

                Tag::Item => {
                    flush_line(&mut lines, &mut current_spans);
                    let indent = "  ".repeat(list_depth);
                    current_spans.push(Span::raw(indent));
                    current_spans.push(Span::styled("• ", Style::default().fg(Color::Yellow)));
                }

                _ => {}
            },

            // ----- End tags -----
            Event::End(tag_end) => match tag_end {
                TagEnd::Heading(_) => {
                    style_stack.pop();
                    flush_line(&mut lines, &mut current_spans);
                    lines.push(Line::default());
                }

                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Link => {
                    style_stack.pop();
                }

                TagEnd::CodeBlock => {
                    in_code_block = false;

                    for code_line in &code_block_lines {
                        lines.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(code_line.clone(), Style::default().fg(Color::Green)),
                        ]));
                    }

                    code_block_lines.clear();
                    lines.push(Line::default());
                }

                TagEnd::Item => {
                    flush_line(&mut lines, &mut current_spans);
                }

                TagEnd::List(_) => {
                    list_depth = list_depth.saturating_sub(1);

                    if list_depth == 0 {
                        lines.push(Line::default());
                    }
                }

                TagEnd::Paragraph => {
                    flush_line(&mut lines, &mut current_spans);
                    lines.push(Line::default());
                }

                _ => {}
            },

            // ----- Content -----
            Event::Text(text) => {
                if in_code_block {
                    // Code blocks: split by newlines to preserve structure
                    for line in text.split('\n') {
                        code_block_lines.push(line.to_string());
                    }
                } else {
                    let style = current_style(&style_stack);
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }

            Event::Code(code) => {
                current_spans.push(Span::styled(
                    code.to_string(),
                    Style::default().fg(Color::Green),
                ));
            }

            Event::SoftBreak => {
                // Soft break = space in normal flow
                if !in_code_block {
                    flush_line(&mut lines, &mut current_spans);
                }
            }

            Event::HardBreak => {
                flush_line(&mut lines, &mut current_spans);
            }

            Event::Rule => {
                flush_line(&mut lines, &mut current_spans);
                lines.push(Line::styled(
                    "─".repeat(60),
                    Style::default().fg(Color::DarkGray),
                ));
                lines.push(Line::default());
            }

            _ => {}
        }
    }

    // Flush remaining content
    flush_line(&mut lines, &mut current_spans);

    lines
}

/// Push current_spans as a Line and clear the buffer.
fn flush_line(lines: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(std::mem::take(spans)));
    }
}

/// Get the current active style from the stack.
fn current_style(stack: &[Style]) -> Style {
    *stack.last().unwrap_or(&Style::default())
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

    #[test]
    fn test_list() {
        let md = "- Item 1\n- Item 2\n  - Nested";
        let lines = render_markdown(md);
        // Should have items with bullet points
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_heading_spacing() {
        let md = "# Title\n\nParagraph text.";
        let lines = render_markdown(md);
        // Title, blank, paragraph, blank
        assert!(lines.len() >= 3);
    }
}
