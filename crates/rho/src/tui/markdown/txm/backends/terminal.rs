use std::fmt;
use std::fmt::Write;

use crate::tui::markdown::txm::backend::Backend;
use crate::tui::markdown::txm::backend::RenderTarget;
use crate::tui::markdown::txm::layout_tree::LayoutNode;
use crate::tui::markdown::txm::style::Style;

use super::generic_backend;

pub struct TerminalBackend;

impl TerminalBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TerminalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for TerminalBackend {
    type Output = String;
    type Error = fmt::Error;

    fn render(&self, tree: &LayoutNode) -> Result<String, fmt::Error> {
        let mut buf = CharBuf::new(tree.width, tree.height);
        generic_backend::render_node(tree, &mut buf, 0, 0);
        Ok(buf.to_string())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CharBuf {
    pub data: Vec<char>,
    pub styles: Vec<Style>,
    pub width: usize,
    pub height: usize,
}

impl CharBuf {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            data: vec![' '; width * height],
            styles: vec![Style::default(); width * height],
            width,
            height,
        }
    }
}

impl fmt::Display for CharBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let style_rows = self.styles.chunks_exact(self.width).take(self.height);
        let data_rows = self.data.chunks_exact(self.width).take(self.height);

        for (style_row, data_row) in style_rows.zip(data_rows) {
            let mut active_style: Option<&Style> = None;

            for (style, &data) in style_row.iter().zip(data_row.iter()) {
                if Some(style) != active_style {
                    if active_style.is_some_and(|s| !s.is_empty()) {
                        f.write_str("\x1b[0m")?;
                    }

                    if !style.is_empty() {
                        style.write_ansi_prefix(f)?;
                    }

                    active_style = Some(style);
                }

                f.write_char(data)?;
            }

            if active_style.is_some_and(|s| !s.is_empty()) {
                f.write_str("\x1b[0m")?;
            }

            writeln!(f)?;
        }

        Ok(())
    }
}

impl RenderTarget for CharBuf {
    fn set(&mut self, x: usize, y: usize, ch: char, style: Style) {
        if x < self.width && y < self.height {
            let i = y * self.width + x;
            self.data[i] = ch;
            self.styles[i] = style;
        }
    }

    fn fill_row(&mut self, y: usize, x_start: usize, x_end: usize, ch: char, style: Style) {
        for x in x_start..x_end.min(self.width) {
            self.set(x, y, ch, style);
        }
    }
}
