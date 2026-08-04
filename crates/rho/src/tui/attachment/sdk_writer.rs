//! Rho runtime attachment recording: SDK events to journal lines.
//!
//! Keeps [`crate::run_artifacts::AttachmentWriter`] free of SDK types.
//!
//! Production Rho runs translate events with [`translate_run_event`] and feed
//! [`crate::run_artifacts::RunArtifactSink`]. [`SdkAttachmentWriter`] is a
//! test-only convenience that owns a journal writer for unit coverage.

use super::super::event_adapter::{SdkEventAdapter, ViewEvent, ViewModelEvent};
use crate::run_artifacts::AttachmentEvent;

/// Test-only helper that records Rho SDK run events into a journal writer.
///
/// Production uses [`translate_run_event`] with [`crate::run_artifacts::RunArtifactSink`].
#[cfg(test)]
pub(crate) struct SdkAttachmentWriter {
    writer: crate::run_artifacts::AttachmentWriter,
    adapter: SdkEventAdapter,
}

#[cfg(test)]
impl SdkAttachmentWriter {
    pub(crate) fn new(
        result_path: &std::path::Path,
        cwd: std::path::PathBuf,
        prompt: &str,
    ) -> anyhow::Result<Self> {
        let mut writer = crate::run_artifacts::AttachmentWriter::create(result_path)?;
        writer.write_event(&AttachmentEvent::Prompt(prompt.to_string()))?;
        Ok(Self {
            writer,
            adapter: SdkEventAdapter::new(cwd),
        })
    }

    pub(crate) fn on_event(&mut self, event: &rho_sdk::RunEvent) -> anyhow::Result<()> {
        for attachment in translate_run_event(&mut self.adapter, event) {
            self.writer.write_event(&attachment)?;
        }
        Ok(())
    }
}

/// Translate one SDK event into zero or more attachment events.
pub(crate) fn translate_run_event(
    adapter: &mut SdkEventAdapter,
    event: &rho_sdk::RunEvent,
) -> Vec<AttachmentEvent> {
    adapter
        .translate(event.clone())
        .into_iter()
        .filter_map(|view_event| match view_event {
            ViewEvent::Update(update) => attachment_update(adapter, update),
            ViewEvent::Notice(notice) => Some(AttachmentEvent::Notice(notice)),
            ViewEvent::Questionnaire { request, .. } => Some(AttachmentEvent::Notice(format!(
                "input requested but unavailable in a delegated agent: {}",
                request.title()
            ))),
            ViewEvent::Completed => {
                adapter.clear_attachment_preview_keys();
                Some(AttachmentEvent::Completed)
            }
            ViewEvent::Cancelled => {
                adapter.clear_attachment_preview_keys();
                Some(AttachmentEvent::Cancelled)
            }
            ViewEvent::Failed(message) => {
                adapter.clear_attachment_preview_keys();
                Some(AttachmentEvent::Failed(message))
            }
        })
        .collect()
}

fn attachment_update(
    adapter: &mut SdkEventAdapter,
    update: ViewModelEvent,
) -> Option<AttachmentEvent> {
    match update {
        ViewModelEvent::OutputDelta(text) => Some(AttachmentEvent::AssistantTextDelta(text)),
        ViewModelEvent::ReasoningDelta(text) => Some(AttachmentEvent::ReasoningDelta(text)),
        ViewModelEvent::ToolStarted { call_id, card }
        | ViewModelEvent::ToolCallProposed { call_id, card } => {
            let key = adapter.attachment_key_for_call(&call_id);
            Some(AttachmentEvent::ToolStarted {
                key: Some(key),
                card,
            })
        }
        ViewModelEvent::ToolCallUpdated {
            index,
            call_id,
            card,
        } => {
            let key = adapter.attachment_key_for_preview(index, call_id.as_ref());
            card.map(|card| AttachmentEvent::ToolStarted {
                key: Some(key),
                card,
            })
        }
        ViewModelEvent::ToolUpdated { call_id, card } => {
            let key = adapter.attachment_key_for_call(&call_id);
            Some(AttachmentEvent::ToolUpdated {
                key: Some(key),
                card,
            })
        }
        ViewModelEvent::ToolFinished { call_id, card, .. } => {
            let key = adapter.take_attachment_key_for_call(&call_id);
            Some(AttachmentEvent::ToolFinished {
                key: Some(key),
                card,
            })
        }
        ViewModelEvent::RunStarted => None,
        ViewModelEvent::StepStarted(_) => Some(AttachmentEvent::StepStarted),
        // This acknowledgement reconciles the interactive TUI's pending-input
        // controls. Read-only attachments have no corresponding state.
        ViewModelEvent::SteeringApplied(_) => None,
        ViewModelEvent::ProviderStreamReset(_) => {
            adapter.clear_attachment_preview_keys();
            Some(AttachmentEvent::ProviderStreamReset)
        }
        ViewModelEvent::ProviderRetry => None,
        ViewModelEvent::ContextUsage(usage) => Some(AttachmentEvent::ContextUsage(usage)),
        ViewModelEvent::Usage(usage) => Some(AttachmentEvent::Usage(usage)),
        ViewModelEvent::ModelCallCompleted { .. } | ViewModelEvent::LiveOutputTokens(_) => None,
    }
}

#[cfg(test)]
#[path = "sdk_writer_tests.rs"]
mod tests;
