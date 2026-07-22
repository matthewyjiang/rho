use std::io;

use rho_providers::model::{image_summary, ImageContent};

use crate::clipboard::{image_from_paste_text, read_clipboard_image, PasteImageOutcome};
pub(super) use crate::clipboard::{CopyOutcome, SystemClipboard};

use super::{App, ComposerMode};

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

impl App {
    pub(super) fn paste_clipboard_image(&mut self) {
        if self.running {
            self.notify_status("image paste is unavailable while a model turn is running");
            return;
        }
        if !matches!(self.composer, ComposerMode::Input) {
            self.notify_status("image paste is only available in the message box");
            return;
        }
        match read_clipboard_image() {
            Ok(image) => self.attach_pending_image(image),
            Err(err) => {
                self.notify_status(format!("image paste failed: {err}"));
            }
        }
    }

    /// Returns true when the paste was consumed as an image path attach attempt.
    pub(super) fn try_attach_pasted_image_path(&mut self, text: &str) -> bool {
        if self.running || !matches!(self.composer, ComposerMode::Input) {
            return false;
        }
        match image_from_paste_text(text, &self.info.runtime.cwd) {
            PasteImageOutcome::NotImage => false,
            PasteImageOutcome::Image(image) => {
                self.attach_pending_image(image);
                true
            }
            PasteImageOutcome::Failed(err) => {
                self.notify_status(format!("image paste failed: {err}"));
                true
            }
        }
    }

    fn attach_pending_image(&mut self, image: ImageContent) {
        let summary = image_summary(&image);
        self.pending_images.push(image);
        self.notify_status(format!(
            "attached image {} ({summary})",
            self.pending_images.len()
        ));
    }
}

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod tests;
