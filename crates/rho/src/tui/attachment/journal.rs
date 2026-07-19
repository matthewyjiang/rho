use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

use rho_sdk::model::{ContextUsage, ModelUsage};
use serde::{Deserialize, Serialize};

use {crate::subagent, rho_tools::tool::ToolDisplayStyle};

use super::super::event_adapter::{
    compaction_completed_notice, SdkEventAdapter, ViewEvent, ViewModelEvent,
    COMPACTION_STARTED_NOTICE,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub(super) enum AttachmentEvent {
    Prompt(String),
    AssistantTextDelta(String),
    ReasoningDelta(String),
    ToolStarted {
        display_lines: Vec<String>,
    },
    ToolUpdated {
        display_lines: Vec<String>,
    },
    ToolFinished {
        ok: bool,
        display_style: ToolDisplayStyle,
        display_lines: Vec<String>,
    },
    Notice(String),
    ContextUsage(ContextUsage),
    Usage(ModelUsage),
    StepStarted,
    ProviderStreamReset,
    Completed,
    Cancelled,
    Failed(String),
}

/// Persists the same presentation events consumed by the interactive TUI so a
/// separate `rho attach` process can render a subagent without controlling it.
pub(crate) struct AttachmentWriter {
    file: File,
    adapter: SdkEventAdapter,
}

impl AttachmentWriter {
    pub(crate) fn new(result_path: &Path, cwd: PathBuf, prompt: &str) -> anyhow::Result<Self> {
        let path = result_path.with_file_name(subagent::ATTACHMENT_FILE_NAME);
        let file = subagent::create_private_file(&path)?;
        let mut writer = Self {
            file,
            adapter: SdkEventAdapter::new(cwd),
        };
        writer.write(&AttachmentEvent::Prompt(prompt.to_string()))?;
        Ok(writer)
    }

    pub(crate) fn on_event(&mut self, event: &rho_sdk::RunEvent) -> anyhow::Result<()> {
        let attachment = match self.adapter.translate(event.clone()) {
            ViewEvent::Update(update) => attachment_update(update),
            ViewEvent::Notice(notice) => Some(AttachmentEvent::Notice(notice)),
            ViewEvent::Questionnaire(request) => Some(AttachmentEvent::Notice(format!(
                "input requested but unavailable in a delegated agent: {}",
                request.title()
            ))),
            ViewEvent::Completed => Some(AttachmentEvent::Completed),
            ViewEvent::Cancelled => Some(AttachmentEvent::Cancelled),
            ViewEvent::Failed(message) => Some(AttachmentEvent::Failed(message)),
            ViewEvent::Ignored => None,
        };
        if let Some(attachment) = attachment {
            self.write(&attachment)?;
        }
        Ok(())
    }

    fn write(&mut self, event: &AttachmentEvent) -> anyhow::Result<()> {
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        Ok(())
    }
}

fn attachment_update(update: ViewModelEvent) -> Option<AttachmentEvent> {
    match update {
        ViewModelEvent::OutputDelta(text) => Some(AttachmentEvent::AssistantTextDelta(text)),
        ViewModelEvent::ReasoningDelta(text) => Some(AttachmentEvent::ReasoningDelta(text)),
        ViewModelEvent::ToolStarted { display_lines }
        | ViewModelEvent::ToolCallUpdated { display_lines } => {
            Some(AttachmentEvent::ToolStarted { display_lines })
        }
        ViewModelEvent::ToolUpdated { display_lines } => {
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
        ViewModelEvent::CompactionStarted => {
            Some(AttachmentEvent::Notice(COMPACTION_STARTED_NOTICE.into()))
        }
        ViewModelEvent::CompactionCompleted {
            previous_messages,
            current_messages,
        } => Some(AttachmentEvent::Notice(compaction_completed_notice(
            previous_messages,
            current_messages,
        ))),
        ViewModelEvent::ContextUsage(usage) => Some(AttachmentEvent::ContextUsage(usage)),
        ViewModelEvent::Usage(usage) => Some(AttachmentEvent::Usage(usage)),
    }
}
pub(super) struct AttachmentReader {
    path: PathBuf,
    file: Option<File>,
    offset: u64,
    pending: Vec<u8>,
}

impl AttachmentReader {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            offset: 0,
            pending: Vec::new(),
        }
    }

    pub(super) fn read_new(&mut self) -> anyhow::Result<Vec<AttachmentEvent>> {
        if self.file.is_none() {
            self.file = match File::open(&self.path) {
                Ok(file) => Some(file),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Vec::new());
                }
                Err(error) => return Err(error.into()),
            };
        }
        let file = self.file.as_mut().expect("attachment file opened");
        if file.metadata()?.len() < self.offset {
            file.seek(std::io::SeekFrom::Start(0))?;
            self.offset = 0;
            self.pending.clear();
        }
        file.seek(std::io::SeekFrom::Start(self.offset))?;
        let mut appended = Vec::new();
        file.read_to_end(&mut appended)?;
        self.offset = self.offset.saturating_add(appended.len() as u64);
        self.pending.extend_from_slice(&appended);

        let Some(complete_end) = self
            .pending
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
        else {
            return Ok(Vec::new());
        };
        let complete: Vec<u8> = self.pending.drain(..complete_end).collect();
        let events = complete
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_slice(line).unwrap_or_else(|error| {
                    AttachmentEvent::Notice(format!("skipped invalid attachment event: {error}"))
                })
            })
            .collect();
        Ok(events)
    }
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
