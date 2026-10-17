//! Replaceable argument previews, not an append-only history of growing cards.
//!
//! The journal is still the authority for tool lifecycles. A snapshot's byte
//! watermark lets readers reject stale previews after a durable tool event or
//! reset, including when the writer dies between a journal and snapshot write.

use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use super::AttachmentEvent;
use crate::{app::interactive_presenter::PresentedToolCard, config_writer};

const FILE_NAME: &str = "tool-previews.json";

#[derive(Default, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct ToolPreviews {
    pub(super) journal_offset: u64,
    // Preserve first-seen order for concurrent calls, including index-only keys.
    pub(super) cards: Vec<(String, PresentedToolCard)>,
}

impl ToolPreviews {
    pub(super) fn upsert(&mut self, key: &str, card: &PresentedToolCard) {
        if let Some((_, previous)) = self.cards.iter_mut().find(|(id, _)| id == key) {
            previous.clone_from(card);
        } else {
            self.cards.push((key.to_owned(), card.clone()));
        }
    }

    pub(super) fn remove(&mut self, key: &Option<String>) {
        if let Some(key) = key {
            self.cards.retain(|(id, _)| id != key);
        }
    }

    pub(super) fn write(&self, journal_path: &Path) -> anyhow::Result<()> {
        config_writer::write_bytes_atomically_with_durability(
            &journal_path.with_file_name(FILE_NAME),
            &serde_json::to_vec(self)?,
            config_writer::WriteDurability::Replaceable,
        )?;
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct PreviewReader {
    // Compare before parsing, and report a broken optional file only once per
    // change. Preview failures must not prevent reading authoritative history.
    last: Option<Result<Vec<u8>, String>>,
    tool_offsets: BTreeMap<String, u64>,
    reset_offset: u64,
}

impl PreviewReader {
    pub(super) fn observe(&mut self, event: &AttachmentEvent, offset: u64) {
        match event {
            AttachmentEvent::ToolStarted { key, .. }
            | AttachmentEvent::ToolUpdated { key, .. }
            | AttachmentEvent::ToolFinished { key, .. } => {
                if let Some(key) = key {
                    self.tool_offsets.insert(key.clone(), offset);
                }
            }
            AttachmentEvent::ProviderStreamReset
            | AttachmentEvent::Completed
            | AttachmentEvent::Cancelled
            | AttachmentEvent::Failed(_) => {
                self.reset_offset = offset;
                self.tool_offsets.clear();
            }
            AttachmentEvent::Prompt(_)
            | AttachmentEvent::Message(_)
            | AttachmentEvent::AssistantTextDelta(_)
            | AttachmentEvent::ReasoningDelta(_)
            | AttachmentEvent::ToolPreview { .. }
            | AttachmentEvent::Notice(_)
            | AttachmentEvent::ContextUsage(_)
            | AttachmentEvent::Usage(_)
            | AttachmentEvent::ModelCallCompleted { .. }
            | AttachmentEvent::StepStarted => {}
        }
    }

    pub(super) fn read_new(
        &mut self,
        journal_path: &Path,
        journal_offset: u64,
    ) -> Vec<AttachmentEvent> {
        let read = match std::fs::read(journal_path.with_file_name(FILE_NAME)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            result => result.map_err(|error| format!("could not read live tool previews: {error}")),
        };
        if self.last.as_ref() == Some(&read) {
            return Vec::new();
        }
        let snapshot = match &read {
            Ok(bytes) => serde_json::from_slice::<ToolPreviews>(bytes)
                .map_err(|error| format!("skipped invalid live tool previews: {error}")),
            Err(error) => Err(error.clone()),
        };
        let events = match snapshot {
            // The writer advanced since this poll read the journal. Retry after
            // the next journal read rather than showing a preview out of order.
            Ok(snapshot) if snapshot.journal_offset > journal_offset => return Vec::new(),
            Ok(snapshot) => snapshot
                .cards
                .into_iter()
                .filter(|(key, _)| {
                    snapshot.journal_offset >= self.reset_offset
                        && snapshot.journal_offset >= *self.tool_offsets.get(key).unwrap_or(&0)
                })
                .map(|(key, card)| AttachmentEvent::ToolPreview { key, card })
                .collect(),
            Err(error) => vec![AttachmentEvent::Notice(error)],
        };
        self.last = Some(read);
        events
    }
}
