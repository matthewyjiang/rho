//! Virtual terminal screen reconstructed from PTY output.

use vt100::Parser;

/// Visible terminal state driven by a VT100 parser.
pub struct ScreenModel {
    parser: Parser<TerminalReplies>,
}

#[derive(Default)]
struct TerminalReplies {
    bytes: Vec<u8>,
    /// The last presented screen while a DEC 2026 synchronized update is open.
    /// A real terminal keeps showing it until the frame ends, so every reader
    /// sees whole frames rather than a half-written one.
    presented: Option<vt100::Screen>,
}

impl vt100::Callbacks for TerminalReplies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        intermediate1: Option<u8>,
        intermediate2: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        if intermediate1 == Some(b'?')
            && intermediate2.is_none()
            && matches!(command, 'h' | 'l')
            && params.contains(&&[2026][..])
        {
            // A begin inside an open update keeps the frame saved at the
            // first begin; Rho's synced backend sends one for a queued clear
            // and another for the draw that follows.
            if command == 'l' {
                self.presented = None;
            } else if self.presented.is_none() {
                self.presented = Some(screen.clone());
            }
        }
        // DSR 6 requests the current cursor in one-based terminal coordinates.
        // ConPTY's INHERIT_CURSOR startup waits for this reply before painting.
        if intermediate1.is_none()
            && intermediate2.is_none()
            && command == 'n'
            && params == [&[6][..]]
        {
            let (row, column) = screen.cursor_position();
            self.bytes.extend_from_slice(
                format!("\x1b[{};{}R", u32::from(row) + 1, u32::from(column) + 1).as_bytes(),
            );
        }
    }
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
            parser: Parser::new_with_callbacks(
                rows,
                cols,
                /*scrollback_len*/ 0,
                TerminalReplies::default(),
            ),
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.parser.process(bytes);
        }
    }

    /// Whether a synchronized update is still open, so readers see the frame
    /// before it rather than the bytes written since.
    pub(crate) fn frame_open(&self) -> bool {
        self.parser.callbacks().presented.is_some()
    }

    /// What a terminal honoring synchronized output would show right now.
    fn presented(&self) -> &vt100::Screen {
        self.parser
            .callbacks()
            .presented
            .as_ref()
            .unwrap_or_else(|| self.parser.screen())
    }

    pub(crate) fn take_terminal_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.parser.callbacks_mut().bytes)
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        if let Some(presented) = &mut self.parser.callbacks_mut().presented {
            presented.set_size(rows, cols);
        }
    }

    pub fn rows(&self) -> u16 {
        self.presented().size().0
    }

    pub fn cols(&self) -> u16 {
        self.presented().size().1
    }

    /// Full visible contents with trailing spaces trimmed per row.
    pub fn contents(&self) -> String {
        self.presented().contents()
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
        self.presented().cursor_position()
    }

    /// Text on the cursor row left of the cursor, where an input field shows
    /// what was just typed.
    pub(crate) fn text_before_cursor(&self) -> String {
        let (row, col) = self.cursor();
        self.presented().contents_between(row, 0, row, col)
    }

    /// Whether the reconstructed terminal has hidden the caret (`DECTCEM` off).
    pub fn hide_cursor(&self) -> bool {
        self.presented().hide_cursor()
    }

    /// Cell at `(row, col)`, if the coordinates fall inside the screen.
    pub(crate) fn cell(&self, row: u16, col: u16) -> Option<ScreenCell> {
        let cell = self.presented().cell(row, col)?;
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
#[path = "screen_query_tests.rs"]
mod query_tests;

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

    // Covers: readers see the last whole frame while a synchronized update is
    // still being written, as a terminal honoring DEC 2026 would.
    #[test]
    fn holds_presented_frame_during_synchronized_update() {
        let mut screen = ScreenModel::new(3, 10);
        let mut shown = Vec::new();
        // A clear and the draw after it each begin the update.
        for chunk in [
            &b"old"[..],
            b"\x1b[?20",
            b"26h\x1b[2J",
            b"\x1b[?2026h\x1b[Hnew",
            b"\x1b[?2026l",
        ] {
            screen.process(chunk);
            shown.push(screen.contents());
        }
        assert_eq!(shown, ["old", "old", "old", "old", "new"]);
    }
}
