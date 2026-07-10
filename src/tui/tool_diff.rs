use super::theme::Theme;
use ratatui::style::Style;

/// Returns the foreground style for one compact file-diff line.
pub(super) fn line_style(line: &str, base: Style) -> Style {
    match line.as_bytes().first() {
        Some(b'+') => Theme::diff_addition(base),
        Some(b'-') => Theme::diff_removal(base),
        Some(_) | None => base,
    }
}

pub(super) fn logical_lines(display_lines: &[String]) -> Vec<String> {
    display_lines
        .iter()
        .flat_map(|line| {
            let lines = line.lines().map(str::to_string).collect::<Vec<_>>();
            if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    #[test]
    fn colors_added_and_removed_lines() {
        assert_eq!(line_style("-old", Style::default()).fg, Some(Color::Red));
        assert_eq!(line_style("+new", Style::default()).fg, Some(Color::Green));
        assert_eq!(line_style("context", Style::default()), Style::default());
    }
}
