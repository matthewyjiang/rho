use std::{future::Future, io, path::PathBuf, pin::Pin, task::Poll};

use rho_providers::model::{image_summary, ImageContent};

use crate::clipboard::{
    paste_text_as_file_path, path_has_supported_image_magic, read_clipboard_image, read_image_file,
};
pub(super) use crate::clipboard::{CopyOutcome, SystemClipboard};

use super::{App, ChatMedia, ChatTextDocument, ComposerMode, MediaAttachId};

pub(super) struct MediaAttachTask {
    id: MediaAttachId,
    task: tokio::task::JoinHandle<PastedMediaOutcome>,
}

impl MediaAttachTask {
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

pub(super) struct CompletedMediaAttach {
    id: MediaAttachId,
    outcome: PastedMediaOutcome,
}

enum PastedImageOutcome {
    NotImage,
    Image(ImageContent),
    Failed { kind: &'static str, message: String },
}

pub(super) async fn next_media_attach_completion(
    pending: &mut Vec<MediaAttachTask>,
) -> CompletedMediaAttach {
    let (index, id, result) = std::future::poll_fn(|context| {
        for (index, pending) in pending.iter_mut().enumerate() {
            if let Poll::Ready(result) = Pin::new(&mut pending.task).poll(context) {
                return Poll::Ready((index, pending.id, result));
            }
        }
        Poll::Pending
    })
    .await;
    let completed = pending.remove(index);
    debug_assert_eq!(completed.id, id);
    CompletedMediaAttach {
        id,
        outcome: result.unwrap_or_else(|error| PastedMediaOutcome::TaskFailed(error.to_string())),
    }
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
    pub(super) fn cancel_all_pending_attachments(&mut self) {
        let ids = self
            .input_ui
            .attachments()
            .iter()
            .filter_map(|attachment| attachment.pending_id())
            .collect::<Vec<_>>();
        for id in ids {
            self.input_ui.remove_pending_attachment(id);
            self.cancel_pending_attachment(id);
        }
        for orphaned_task in self.media_attach_tasks.drain(..) {
            orphaned_task.cancel();
        }
    }

    pub(super) fn cancel_pending_attachment(&mut self, id: MediaAttachId) -> bool {
        let Some(index) = self
            .media_attach_tasks
            .iter()
            .position(|pending| pending.id == id)
        else {
            return false;
        };
        let pending = self.media_attach_tasks.remove(index);
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
            Ok(image) => self.attach_ready_image(image),
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
        let id = MediaAttachId::new();
        let task = tokio::spawn(classify_pasted_path(path, original_text));
        self.media_attach_tasks.push(MediaAttachTask { id, task });
        self.input_ui.push_pending_attachment(id, name.clone());
        self.notify_status(format!("extracting {name}"));
        true
    }

    pub(super) fn finish_pasted_media(&mut self, completion: CompletedMediaAttach) {
        let CompletedMediaAttach { id, outcome } = completion;
        match outcome {
            PastedMediaOutcome::Unsupported { original_text } => {
                if self.input_ui.remove_pending_attachment(id).is_some() {
                    self.insert_pasted_input_text(&original_text);
                }
            }
            PastedMediaOutcome::Image(image) => self.finish_pending_image(id, image),
            PastedMediaOutcome::Document(document) => {
                self.finish_pending_document(id, document);
            }
            PastedMediaOutcome::Failed { kind, message } => {
                if self.input_ui.remove_pending_attachment(id).is_some() {
                    self.notify_status(format!("{kind} paste failed: {message}"));
                }
            }
            PastedMediaOutcome::TaskFailed(message) => {
                if self.input_ui.remove_pending_attachment(id).is_some() {
                    self.notify_status(format!("file paste task failed: {message}"));
                }
            }
        }
    }

    fn finish_pending_document(
        &mut self,
        id: MediaAttachId,
        document: rho_tools::document::ExtractedDocument,
    ) {
        let media = ChatMedia::TextDocument(ChatTextDocument::from(document));
        let label = media.composer_label(1);
        if self
            .input_ui
            .replace_pending_attachment(id, media)
            .is_some()
        {
            self.notify_status(format!("attached {label}"));
        }
    }

    fn attach_ready_image(&mut self, image: ImageContent) {
        let summary = image_summary(&image);
        self.input_ui.push_ready_attachment(ChatMedia::Image(image));
        self.notify_status(format!(
            "attached image {} ({summary})",
            self.input_ui.attachments().len()
        ));
    }

    fn finish_pending_image(&mut self, id: MediaAttachId, image: ImageContent) {
        let summary = image_summary(&image);
        if let Some(index) = self
            .input_ui
            .replace_pending_attachment(id, ChatMedia::Image(image))
        {
            self.notify_status(format!("attached image {} ({summary})", index + 1));
        }
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
