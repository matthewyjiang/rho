//! Display-only release policy. Provider events and persisted output stay unchanged.

use super::App;

pub(super) use crate::config::StreamingMode;

impl App {
    pub(super) fn cycle_streaming_mode(&mut self) {
        self.set_streaming_mode(self.streams.mode.next());
    }

    pub(super) fn set_streaming_mode(&mut self, mode: StreamingMode) {
        if !self.save_streaming_mode(mode) {
            return;
        }
        if self.streams.mode != mode {
            // Commit received text once at the switch, never retract visible output.
            self.finish_current_stream();
            self.streams.mode = mode;
        }
        self.set_status(format!("output streaming: {}", mode.label()));
    }
}

/// Incrementally finds blank-line boundaries, including whitespace-only CRLF lines.
/// Only newly appended bytes are scanned, even for a long unfinished paragraph.
#[derive(Default)]
pub(super) struct ParagraphBoundary {
    scanned: usize,
    line_has_text: bool,
    saw_newline: bool,
}

impl ParagraphBoundary {
    /// The caller must drain the returned prefix before appending more text.
    pub(super) fn release_end(&mut self, text: &str) -> usize {
        let mut end = 0;
        for (offset, byte) in text.as_bytes()[self.scanned..].iter().enumerate() {
            match byte {
                b'\n' => {
                    if self.saw_newline && !self.line_has_text {
                        end = self.scanned + offset + 1;
                    }
                    self.saw_newline = true;
                    self.line_has_text = false;
                }
                b' ' | b'\t' | b'\r' => {}
                _ => self.line_has_text = true,
            }
        }
        self.scanned = text.len() - end;
        end
    }
}

#[cfg(test)]
#[path = "streaming_mode_tests.rs"]
mod tests;
