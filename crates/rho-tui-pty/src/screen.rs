//! Virtual terminal screen reconstructed from PTY output.

use vt100::Parser;

/// Visible terminal state driven by a VT100 parser.
pub struct ScreenModel {
    parser: Parser,
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
