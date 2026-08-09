//! Attaching media to the composer while the work happens off the input loop.
//!
//! A pasted file path and a picked MCP resource both take long enough that the
//! composer must not block on them, and both must show up the instant the user
//! acts. So both push a pending attachment, run a task, and replace or remove
//! that entry when the task ends. Those mechanics live here; what each source
//! produces is its own module's policy.
//!
//! Submission is gated on there being no pending attachment, so every path out
//! of a task must either replace the entry or remove it. A task that ends any
//! other way would leave the composer unable to send.

use std::{future::Future, pin::Pin, task::Poll};

use super::{App, ChatMedia, MediaAttachId};

/// A running attach, paired with the composer entry it will settle.
pub(super) struct MediaAttachTask {
    pub(super) id: MediaAttachId,
    pub(super) task: tokio::task::JoinHandle<MediaAttachOutcome>,
}

impl MediaAttachTask {
    fn cancel(self) {
        self.task.abort();
    }
}

/// How one attach ended.
pub(super) enum MediaAttachOutcome {
    /// Media to put in place of the pending entry.
    Ready(ChatMedia),
    /// Nothing was attachable. The source's original text goes back into the
    /// composer so the user does not lose what they pasted.
    Unsupported { original_text: String },
    /// The attach failed. `kind` names the thing that failed, so the status
    /// reads as a sentence.
    Failed { kind: &'static str, message: String },
}

pub(super) struct CompletedMediaAttach {
    pub(super) id: MediaAttachId,
    pub(super) outcome: MediaAttachOutcome,
}

/// Waits for whichever attach finishes first and takes it off the list.
///
/// Cancellation safe: nothing is removed until a task has actually produced a
/// value, so dropping this future in a `select!` loses no work.
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
        outcome: result.unwrap_or_else(|error| MediaAttachOutcome::Failed {
            kind: "attachment task",
            message: error.to_string(),
        }),
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

    /// Settle the composer entry this attach owns.
    ///
    /// Every arm ends with the pending entry gone, and each one checks that the
    /// entry is still there: the user may have cancelled it with backspace while
    /// the task ran, and a cancelled attach must stay cancelled.
    pub(super) fn finish_media_attach(&mut self, completion: CompletedMediaAttach) {
        let CompletedMediaAttach { id, outcome } = completion;
        match outcome {
            MediaAttachOutcome::Ready(ChatMedia::Image(image)) => {
                self.finish_pending_image(id, image);
            }
            MediaAttachOutcome::Ready(media @ ChatMedia::TextDocument(_)) => {
                let label = media.composer_label(1);
                if self
                    .input_ui
                    .replace_pending_attachment(id, media)
                    .is_some()
                {
                    self.notify_status(format!("attached {label}"));
                }
            }
            MediaAttachOutcome::Unsupported { original_text } => {
                if self.input_ui.remove_pending_attachment(id).is_some() {
                    self.insert_pasted_input_text(&original_text);
                }
            }
            MediaAttachOutcome::Failed { kind, message } => {
                if self.input_ui.remove_pending_attachment(id).is_some() {
                    self.notify_status(format!("{kind} failed: {message}"));
                }
            }
        }
    }
}
