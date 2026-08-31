//! Virtual terminal screen reconstructed from PTY output.

use vt100::Parser;

/// Visible terminal state driven by a VT100 parser.
pub struct ScreenModel {
    parser: Parser,
}

/// Terminal color as recovered from the VT stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CellColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// One reconstructed cell, owned and free of the `vt100` type surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScreenCell {
    pub contents: String,
    pub wide: bool,
    pub wide_continuation: bool,
    pub fg: CellColor,
    pub bg: CellColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl ScreenModel {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Parser::new(rows, cols, 0),
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.parser.process(bytes);
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }

    pub fn rows(&self) -> u16 {
        self.parser.screen().size().0
    }

    pub fn cols(&self) -> u16 {
        self.parser.screen().size().1
    }

    /// Full visible contents with trailing spaces trimmed per row.
    pub fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    /// Visible rows as individual strings with trailing spaces trimmed.
    pub fn rows_text(&self) -> Vec<String> {
        let contents = self.contents();
        if contents.is_empty() {
            return Vec::new();
        }
        contents.lines().map(|line| line.to_string()).collect()
    }

    pub fn cursor(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    /// Whether the reconstructed terminal has hidden the caret (`DECTCEM` off).
    pub fn hide_cursor(&self) -> bool {
        self.parser.screen().hide_cursor()
    }

    /// Cell at `(row, col)`, if the coordinates fall inside the screen.
    pub(crate) fn cell(&self, row: u16, col: u16) -> Option<ScreenCell> {
        let cell = self.parser.screen().cell(row, col)?;
        Some(ScreenCell {
            contents: cell.contents().to_string(),
            wide: cell.is_wide(),
            wide_continuation: cell.is_wide_continuation(),
            fg: map_color(cell.fgcolor()),
            bg: map_color(cell.bgcolor()),
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        })
    }

    /// Columns in `row` rendered with the inverse (reverse-video) attribute.
    pub fn inverse_columns(&self, row: u16) -> Vec<u16> {
        (0..self.cols())
            .filter(|&col| self.cell(row, col).is_some_and(|cell| cell.inverse))
            .collect()
    }

    pub fn contains_text(&self, needle: &str) -> bool {
        self.contents().contains(needle)
    }

    /// Compact one-line debug dump of the visible screen.
    pub fn debug_dump(&self) -> String {
        let mut lines = self.rows_text();
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
        if lines.is_empty() {
            return "<empty screen>".into();
        }
        lines.join("\n")
    }
}

fn map_color(color: vt100::Color) -> CellColor {
    match color {
        vt100::Color::Default => CellColor::Default,
        vt100::Color::Idx(index) => CellColor::Indexed(index),
        vt100::Color::Rgb(r, g, b) => CellColor::Rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_plain_text_and_cursor() {
        let mut screen = ScreenModel::new(4, 20);
        screen.process(b"hello world");
        assert!(screen.contains_text("hello world"));
        assert_eq!(screen.cursor(), (0, 11));
    }

    #[test]
    fn handles_split_escape_sequences() {
        let mut screen = ScreenModel::new(3, 10);
        screen.process(b"\x1b[");
        screen.process(b"2J\x1b[H");
        screen.process(b"ab");
        screen.process(b"c");
        assert!(screen.contains_text("abc"));
    }
}
