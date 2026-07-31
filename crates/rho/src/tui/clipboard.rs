use std::{collections::VecDeque, io, path::PathBuf};

use rho_providers::model::{image_summary, ImageContent};

use crate::clipboard::{
    paste_text_as_file_path, path_has_supported_image_magic, read_clipboard_image, read_image_file,
};
pub(super) use crate::clipboard::{CopyOutcome, SystemClipboard};

use super::{App, ChatMedia, ChatTextDocument, ComposerMode};

pub(super) struct PendingMediaAttach {
    task: tokio::task::JoinHandle<PastedMediaOutcome>,
}

impl PendingMediaAttach {
    fn cancel(self) {
        self.task.abort();
    }
}

pub(super) enum PastedMediaOutcome {
    Unsupported { original_text: String },
    Image(ImageContent),
    Document(rho_tools::document::ExtractedDocument),
    Failed { kind: &'static str, message: String },
    TaskFailed(String),
}

enum PastedImageOutcome {
    NotImage,
    Image(ImageContent),
    Failed { kind: &'static str, message: String },
}

pub(super) async fn next_pending_media_attach(
    pending: &mut VecDeque<PendingMediaAttach>,
) -> PastedMediaOutcome {
    let result = {
        let task = &mut pending
            .front_mut()
            .expect("pending media attachment checked before polling")
            .task;
        task.await
    };
    pending.pop_front();
    result.unwrap_or_else(|error| PastedMediaOutcome::TaskFailed(error.to_string()))
}

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
    pub(super) fn cancel_pending_media_attaches(&mut self) {
        for pending in self.pending_media_attaches.drain(..) {
            pending.cancel();
        }
    }

    pub(super) fn cancel_last_pending_media_attach(&mut self) -> bool {
        let Some(pending) = self.pending_media_attaches.pop_back() else {
            return false;
        };
        pending.cancel();
        true
    }

    pub(super) fn paste_clipboard_image(&mut self) {
        if self.is_ui_busy() {
            self.notify_status("image paste is unavailable while a model turn is running");
            return;
        }
        if !matches!(self.input_ui.composer(), ComposerMode::Input) {
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

    /// Starts background extraction when the paste is a path to a regular file.
    pub(super) fn start_pasted_media_path(&mut self, text: &str) -> bool {
        if self.is_ui_busy() || !matches!(self.input_ui.composer(), ComposerMode::Input) {
            return false;
        }
        let Some(path) = paste_text_as_file_path(text, &self.info.runtime.cwd) else {
            return false;
        };
        let original_text = text.to_owned();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let task = tokio::spawn(classify_pasted_path(path, original_text));
        self.pending_media_attaches
            .push_back(PendingMediaAttach { task });
        self.notify_status(format!("extracting {name}"));
        true
    }

    pub(super) fn finish_pasted_media(&mut self, outcome: PastedMediaOutcome) {
        match outcome {
            PastedMediaOutcome::Unsupported { original_text } => {
                self.insert_pasted_input_text(&original_text);
            }
            PastedMediaOutcome::Image(image) => self.attach_pending_image(image),
            PastedMediaOutcome::Document(document) => self.attach_pending_document(document),
            PastedMediaOutcome::Failed { kind, message } => {
                self.notify_status(format!("{kind} paste failed: {message}"));
            }
            PastedMediaOutcome::TaskFailed(message) => {
                self.notify_status(format!("file paste task failed: {message}"));
            }
        }
    }

    fn attach_pending_document(&mut self, document: rho_tools::document::ExtractedDocument) {
        let media = ChatMedia::TextDocument(ChatTextDocument::from(document));
        let label = media.composer_label(self.input_ui.pending_media().len() + 1);
        self.input_ui.pending_media_mut().push(media);
        self.notify_status(format!("attached {label}"));
    }

    fn attach_pending_image(&mut self, image: ImageContent) {
        let summary = image_summary(&image);
        self.input_ui
            .pending_media_mut()
            .push(ChatMedia::Image(image));
        self.notify_status(format!(
            "attached image {} ({summary})",
            self.input_ui.pending_media().len()
        ));
    }
}

async fn classify_pasted_path(path: PathBuf, original_text: String) -> PastedMediaOutcome {
    let path = match tokio::task::spawn_blocking(move || {
        path.canonicalize().ok().filter(|path| path.is_file())
    })
    .await
    {
        Ok(Some(path)) => path,
        Ok(None) | Err(_) => return PastedMediaOutcome::Unsupported { original_text },
    };
    let image_path = path.clone();
    let image_outcome =
        tokio::task::spawn_blocking(move || classify_pasted_image(image_path)).await;
    match image_outcome {
        Ok(PastedImageOutcome::Image(image)) => PastedMediaOutcome::Image(image),
        Ok(PastedImageOutcome::Failed { kind, message }) => {
            PastedMediaOutcome::Failed { kind, message }
        }
        Ok(PastedImageOutcome::NotImage) => {
            match rho_tools::document::extract_document_from_path_async(path).await {
                Ok(document) => PastedMediaOutcome::Document(document),
                Err(rho_tools::document::DocumentExtractionError::UnsupportedFormat { .. }) => {
                    PastedMediaOutcome::Unsupported { original_text }
                }
                Err(error) => PastedMediaOutcome::Failed {
                    kind: "document",
                    message: error.to_string(),
                },
            }
        }
        Err(error) => PastedMediaOutcome::Failed {
            kind: "file",
            message: error.to_string(),
        },
    }
}

fn classify_pasted_image(path: PathBuf) -> PastedImageOutcome {
    match path_has_supported_image_magic(&path) {
        Ok(true) => match read_image_file(&path) {
            Ok(image) => PastedImageOutcome::Image(image),
            Err(error) => PastedImageOutcome::Failed {
                kind: "image",
                message: error.to_string(),
            },
        },
        Ok(false) => PastedImageOutcome::NotImage,
        Err(error) => PastedImageOutcome::Failed {
            kind: "file",
            message: error.to_string(),
        },
    }
}

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod tests;
