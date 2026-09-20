use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::session::LogEntry;

/// Wraps on spaces, hard-breaks words wider than `width`
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut line_w = 0usize;
        for word in paragraph.split(' ') {
            let word_w: usize = word.chars().map(|c| c.width().unwrap_or(0)).sum();
            let sep = usize::from(!line.is_empty());
            if line_w + sep + word_w <= width {
                if sep == 1 {
                    line.push(' ');
                }
                line.push_str(word);
                line_w += sep + word_w;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                line_w = 0;
            }
            if word_w <= width {
                line.push_str(word);
                line_w = word_w;
            } else {
                for c in word.chars() {
                    let cw = c.width().unwrap_or(0);
                    if line_w + cw > width && !line.is_empty() {
                        lines.push(std::mem::take(&mut line));
                        line_w = 0;
                    }
                    line.push(c);
                    line_w += cw;
                }
            }
        }
        lines.push(line);
    }
    lines
}

pub fn render_log(entries: &[LogEntry], width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            LogEntry::Chat { from, text, mine } => {
                let name_style = if *mine {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD)
                };
                let prefix = format!("{from}: ");
                let prefix_w: usize = prefix.chars().map(|c| c.width().unwrap_or(0)).sum();
                let body_w = width.saturating_sub(prefix_w).max(8);
                for (i, l) in wrap(text, body_w).into_iter().enumerate() {
                    if i == 0 {
                        out.push(Line::from(vec![
                            Span::styled(prefix.clone(), name_style),
                            Span::raw(l),
                        ]));
                    } else {
                        out.push(Line::from(vec![
                            Span::raw(" ".repeat(prefix_w.min(width))),
                            Span::raw(l),
                        ]));
                    }
                }
            }
            LogEntry::Notice(text) => {
                let style = Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC);
                for l in wrap(&format!("* {text}"), width) {
                    out.push(Line::from(Span::styled(l, style)));
                }
            }
            LogEntry::Error(text) => {
                let style = Style::default().fg(Color::Red);
                for l in wrap(&format!("! {text}"), width) {
                    out.push(Line::from(Span::styled(l, style)));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_on_spaces_and_hard_breaks() {
        assert_eq!(wrap("hello world", 11), vec!["hello world"]);
        assert_eq!(wrap("hello world", 10), vec!["hello", "world"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap("", 4), vec![""]);
        assert_eq!(wrap("한글 테스트", 6), vec!["한글", "테스트"]);
        assert_eq!(wrap("한글 테스트", 5), vec!["한글", "테스", "트"]);
    }
}
