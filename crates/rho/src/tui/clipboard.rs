use std::io;

pub(super) use crate::clipboard::{CopyOutcome, SystemClipboard};

/// Writes transcript text to the user's clipboard synchronously.
///
/// Implementors must preserve the supplied text and report whether the destination confirmed the
/// write. Errors mean that no available backend accepted the request.
pub(super) trait ClipboardWriter {
    fn copy(&mut self, text: &str) -> io::Result<CopyOutcome>;
}

impl ClipboardWriter for SystemClipboard {
    fn copy(&mut self, text: &str) -> io::Result<CopyOutcome> {
        self.copy_text(text)
    }
}
