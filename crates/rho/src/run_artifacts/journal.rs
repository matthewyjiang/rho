use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

use rho_sdk::model::{ContextUsage, ModelUsage};
use serde::{Deserialize, Serialize};

use {crate::subagent, rho_tools::tool::ToolDisplayStyle};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub(crate) enum AttachmentEvent {
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

/// Generic attachment journal mechanics for `rho attach`.
///
/// Owns private file creation, `AttachmentEvent` serialization, and per-write
/// flush. Runtime-specific translation (SDK events, Claude stream-json) stays
/// outside this type so callers cannot mix event vocabularies by accident.
pub(crate) struct AttachmentWriter {
    file: File,
}

impl AttachmentWriter {
    /// Open `events.jsonl` beside `result_path` as a private file.
    pub(crate) fn create(result_path: &Path) -> anyhow::Result<Self> {
        let path = result_path.with_file_name(subagent::ATTACHMENT_FILE_NAME);
        let file = subagent::create_private_file(&path)?;
        Ok(Self { file })
    }

    /// Serialize one presentation event and flush so attach readers see it.
    pub(crate) fn write_event(&mut self, event: &AttachmentEvent) -> anyhow::Result<()> {
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        self.file.flush()?;
        Ok(())
    }
}

pub(crate) struct AttachmentReader {
    path: PathBuf,
    file: Option<File>,
    offset: u64,
    pending: Vec<u8>,
}

impl AttachmentReader {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            offset: 0,
            pending: Vec::new(),
        }
    }

    pub(crate) fn read_new(&mut self) -> anyhow::Result<Vec<AttachmentEvent>> {
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
