//! Host-owned display records, kept separate from provider history.
//!
//! Session storage currently carries display messages in the model's Message
//! container. Only the codec below uses that container; presenters keep typed
//! rows, and real human prompts remain separate User messages.

use rho_sdk::model::Message;
use serde::{Deserialize, Serialize};

use crate::presentation::MessageCard;

const PREFIX: &str = "[rho boundary transcript v1]\n";
const FAMILY: &str = "[rho boundary transcript ";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum DisplayRow {
    Message(Box<MessageCard>),
    Notice(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DisplayTranscript(pub(crate) Vec<DisplayRow>);

impl DisplayTranscript {
    pub(crate) fn display_message(&self) -> Message {
        Message::System(format!(
            "{PREFIX}{}",
            serde_json::to_string(self).expect("display rows contain only serializable data")
        ))
    }

    /// Unknown or damaged display records stay visible as a recoverable notice.
    /// Ordinary system text is not interpreted as host display metadata.
    pub(crate) fn from_display(text: &str) -> Option<Self> {
        if !text.starts_with(FAMILY) {
            return None;
        }
        Some(text.strip_prefix(PREFIX)
            .and_then(|body| serde_json::from_str(body).ok())
            .unwrap_or_else(|| Self(vec![DisplayRow::Notice(
                "could not restore notification display: unsupported or malformed saved record; the original record remains in the session file".into(),
            )])))
    }

    /// Plain projection for hosts without expandable message-card rendering.
    pub(crate) fn plain_text(&self) -> String {
        self.0
            .iter()
            .map(|row| match row {
                DisplayRow::Message(card) => {
                    let reference = card
                        .reference
                        .as_deref()
                        .map(|id| format!(" · {id}"))
                        .unwrap_or_default();
                    format!(
                        "{}\n{} → {}{reference}\n{}",
                        card.title, card.sender, card.recipient, card.body
                    )
                }
                DisplayRow::Notice(text) => text.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[cfg(test)]
#[path = "display_transcript_tests.rs"]
mod tests;
