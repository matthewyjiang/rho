use std::{
    fs::File,
    io::{BufRead, Read, Seek, Write},
    path::{Path, PathBuf},
};

use rho_sdk::model::{ContextUsage, ModelUsage};
use serde::{Deserialize, Serialize};

use crate::{app::interactive_presenter::PresentedToolCard, subagent};

use super::previews::{PreviewReader, ToolPreviews};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub(crate) enum AttachmentEvent {
    Prompt(String),
    Message(Box<crate::presentation::NotificationCard>),
    AssistantTextDelta(String),
    ReasoningDelta(String),
    /// Delivery-only growing argument card. Never serialized into events.jsonl.
    #[serde(skip)]
    ToolPreview {
        key: String,
        card: PresentedToolCard,
    },
    ToolStarted {
        /// Stable tool identity for concurrent live cards. Absent on legacy journals.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        card: PresentedToolCard,
    },
    ToolUpdated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        card: PresentedToolCard,
    },
    ToolFinished {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        // Retain the legacy {card: ...} wire shape; messages write {message: ...}.
        #[serde(flatten)]
        presentation: crate::presentation::Presentation,
    },
    Notice(String),
    ContextUsage(ContextUsage),
    Usage(ModelUsage),
    /// Completed model call that streamed generated output. The writer resolves
    /// token attribution (reported vs aggregate fallback) before journaling, so
    /// readers only see usable numbers; calls without both resolved tokens and
    /// generation time are not journaled. Absent for claude-cli runs and old
    /// journals. Old readers render unknown variants as a skipped-event notice.
    ModelCallCompleted {
        generation_output_tokens: u64,
        generation_time_ms: u64,
    },
    StepStarted,
    ProviderStreamReset,
    Completed,
    Cancelled,
    Failed(String),
}

/// Generic attachment journal mechanics for `rho attach`.
///
/// Owns private file creation, durable event appends, and replaceable previews.
/// Runtime-specific translation (SDK events, Claude stream-json) stays
/// outside this type so callers cannot mix event vocabularies by accident.
pub(crate) struct AttachmentWriter {
    file: File,
    path: PathBuf,
    previews: ToolPreviews,
    preview_publication: PreviewPublication,
}

#[derive(PartialEq, Eq)]
enum PreviewPublication {
    Unused,
    Published,
    Disabled,
}

impl AttachmentWriter {
    /// Open `events.jsonl` beside `result_path` as a private file.
    pub(crate) fn create(result_path: &Path) -> anyhow::Result<Self> {
        let path = result_path.with_file_name(subagent::ATTACHMENT_FILE_NAME);
        let file = subagent::create_private_file(&path)?;
        Ok(Self {
            file,
            path,
            previews: ToolPreviews::default(),
            preview_publication: PreviewPublication::Unused,
        })
    }

    /// Deliver previews through a replaceable snapshot; append durable events.
    pub(crate) fn write_event(&mut self, event: &AttachmentEvent) -> anyhow::Result<()> {
        match event {
            AttachmentEvent::ToolPreview { key, card } => {
                self.previews.upsert(key, card);
                return self.publish_previews();
            }
            AttachmentEvent::ToolStarted { key, .. } | AttachmentEvent::ToolUpdated { key, .. } => {
                self.previews.remove(key)
            }
            AttachmentEvent::ToolFinished { key, .. } => {
                // Claude can stream input after its initial start. Preserve the
                // latest input once even when no complete envelope followed it.
                if let Some((_, card)) = self
                    .previews
                    .cards
                    .iter()
                    .find(|(id, _)| Some(id) == key.as_ref())
                {
                    self.append(&AttachmentEvent::ToolUpdated {
                        key: key.clone(),
                        card: card.clone(),
                    })?;
                }
                self.previews.remove(key);
            }
            AttachmentEvent::ProviderStreamReset => self.previews.cards.clear(),
            AttachmentEvent::Completed
            | AttachmentEvent::Cancelled
            | AttachmentEvent::Failed(_) => self.flush_previews()?,
            AttachmentEvent::Prompt(_)
            | AttachmentEvent::Message(_)
            | AttachmentEvent::AssistantTextDelta(_)
            | AttachmentEvent::ReasoningDelta(_)
            | AttachmentEvent::Notice(_)
            | AttachmentEvent::ContextUsage(_)
            | AttachmentEvent::Usage(_)
            | AttachmentEvent::ModelCallCompleted { .. }
            | AttachmentEvent::StepStarted => {}
        }
        self.append(event)?;
        if matches!(
            event,
            AttachmentEvent::ProviderStreamReset
                | AttachmentEvent::Completed
                | AttachmentEvent::Cancelled
                | AttachmentEvent::Failed(_)
        ) {
            self.clear_preview_file()?;
        }
        Ok(())
    }

    /// Preserve the latest partial input once on orderly interruption/drop.
    /// An abrupt exit leaves the replaceable snapshot available to attach.
    pub(crate) fn flush_previews(&mut self) -> anyhow::Result<()> {
        for (key, card) in std::mem::take(&mut self.previews.cards) {
            self.append(&AttachmentEvent::ToolStarted {
                key: Some(key),
                card,
            })?;
        }
        self.clear_preview_file()
    }

    /// A failed replaceable transport must not disable the append-only journal.
    /// Keep the latest in-memory cards for boundary flushing and warn once.
    fn publish_previews(&mut self) -> anyhow::Result<()> {
        if self.preview_publication == PreviewPublication::Disabled {
            return Ok(());
        }
        match self.previews.write(&self.path) {
            Ok(()) => self.preview_publication = PreviewPublication::Published,
            Err(error) => {
                self.preview_publication = PreviewPublication::Disabled;
                self.append(&AttachmentEvent::Notice(format!(
                    "could not update live tool previews; history recording continues: {error}"
                )))?;
            }
        }
        Ok(())
    }

    fn clear_preview_file(&mut self) -> anyhow::Result<()> {
        if self.preview_publication == PreviewPublication::Published {
            self.publish_previews()?;
            if self.preview_publication == PreviewPublication::Published {
                self.preview_publication = PreviewPublication::Unused;
            }
        }
        Ok(())
    }

    fn append(&mut self, event: &AttachmentEvent) -> anyhow::Result<()> {
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        self.previews.journal_offset += line.len() as u64;
        Ok(())
    }
}

/// Task prompt a run was launched with: the journal's first event, which the
/// sink writes before anything else. `None` when the journal is missing,
/// unreadable, or starts with another event.
pub(crate) fn read_prompt(run_directory: &Path) -> Option<String> {
    let file = File::open(run_directory.join(subagent::ATTACHMENT_FILE_NAME)).ok()?;
    let mut first = String::new();
    std::io::BufReader::new(file).read_line(&mut first).ok()?;
    match serde_json::from_str(&first).ok()? {
        AttachmentEvent::Prompt(prompt) => Some(prompt),
        _ => None,
    }
}

pub(crate) struct AttachmentReader {
    path: PathBuf,
    file: Option<File>,
    offset: u64,
    pending: Vec<u8>,
    previews: PreviewReader,
}

impl AttachmentReader {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            offset: 0,
            pending: Vec::new(),
            previews: PreviewReader::default(),
        }
    }

    pub(crate) fn read_new(&mut self) -> anyhow::Result<Vec<AttachmentEvent>> {
        let mut events = self.read_journal()?;
        events.extend(
            self.previews
                .read_new(&self.path, self.offset - self.pending.len() as u64),
        );
        Ok(events)
    }

    fn read_journal(&mut self) -> anyhow::Result<Vec<AttachmentEvent>> {
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
            self.previews = PreviewReader::default();
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
        let mut event_offset = self.offset - self.pending.len() as u64;
        let complete: Vec<u8> = self.pending.drain(..complete_end).collect();
        let mut events = Vec::new();
        for line in complete.split_inclusive(|byte| *byte == b'\n') {
            event_offset += line.len() as u64;
            if line.len() > 1 {
                let event = serde_json::from_slice(line).unwrap_or_else(|error| {
                    AttachmentEvent::Notice(format!("skipped invalid attachment event: {error}"))
                });
                self.previews.observe(&event, event_offset);
                events.push(event);
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "preview_replay_tests.rs"]
mod preview_replay_tests;
