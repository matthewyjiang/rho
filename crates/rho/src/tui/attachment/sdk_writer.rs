//! Rho runtime attachment recording: SDK events to journal lines.
//!
//! Keeps [`crate::run_artifacts::AttachmentWriter`] free of SDK types.
//!
//! Production Rho runs translate events with [`translate_run_event`] and feed
//! [`crate::run_artifacts::RunArtifactSink`]. [`SdkAttachmentWriter`] is a
//! test-only convenience that owns a journal writer for unit coverage.

use super::super::{
    compaction_display::running_display_lines,
    event_adapter::{SdkEventAdapter, ViewEvent, ViewModelEvent},
};
use crate::run_artifacts::AttachmentEvent;
use rho_tools::tool::ToolDisplayStyle;

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
            ViewEvent::Update(update) => attachment_update(update),
            ViewEvent::Notice(notice) => Some(AttachmentEvent::Notice(notice)),
            ViewEvent::Questionnaire { request, .. } => Some(AttachmentEvent::Notice(format!(
                "input requested but unavailable in a delegated agent: {}",
                request.title()
            ))),
            ViewEvent::Completed => Some(AttachmentEvent::Completed),
            ViewEvent::Cancelled => Some(AttachmentEvent::Cancelled),
            ViewEvent::Failed(message) => Some(AttachmentEvent::Failed(message)),
        })
        .collect()
}

fn attachment_update(update: ViewModelEvent) -> Option<AttachmentEvent> {
    match update {
        ViewModelEvent::OutputDelta(text) => Some(AttachmentEvent::AssistantTextDelta(text)),
        ViewModelEvent::ReasoningDelta(text) => Some(AttachmentEvent::ReasoningDelta(text)),
        ViewModelEvent::ToolStarted { display_lines, .. }
        | ViewModelEvent::ToolCallUpdated { display_lines, .. } => {
            Some(AttachmentEvent::ToolStarted { display_lines })
        }
        ViewModelEvent::ToolUpdated { display_lines, .. } => {
            Some(AttachmentEvent::ToolUpdated { display_lines })
        }
        ViewModelEvent::ToolFinished {
            ok,
            display_style,
            display_lines,
            ..
        } => Some(AttachmentEvent::ToolFinished {
            ok,
            display_style,
            display_lines,
        }),
        ViewModelEvent::RunStarted => None,
        ViewModelEvent::StepStarted(_) => Some(AttachmentEvent::StepStarted),
        // This acknowledgement reconciles the interactive TUI's pending-input
        // controls. Read-only attachments have no corresponding state.
        ViewModelEvent::SteeringApplied(_) => None,
        ViewModelEvent::ProviderStreamReset => Some(AttachmentEvent::ProviderStreamReset),
        ViewModelEvent::ProviderRetry => None,
        ViewModelEvent::CompactionStarted => Some(AttachmentEvent::ToolStarted {
            display_lines: running_display_lines(),
        }),
        ViewModelEvent::CompactionFinished { outcome } => Some(AttachmentEvent::ToolFinished {
            ok: outcome.ok(),
            display_style: ToolDisplayStyle::default_tool(),
            display_lines: outcome.display_lines(),
        }),
        ViewModelEvent::ContextUsage(usage) => Some(AttachmentEvent::ContextUsage(usage)),
        ViewModelEvent::Usage(usage) => Some(AttachmentEvent::Usage(usage)),
    }
}

#[cfg(test)]
#[path = "sdk_writer_tests.rs"]
mod tests;
