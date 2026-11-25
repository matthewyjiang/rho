//! Host-owned display records, kept separate from provider history.
//!
//! Session storage currently carries display messages in the model's Message
//! container. Only the codec below uses that container; presenters keep typed
//! rows, and real human prompts remain separate User messages.
//! This private storage adapter can be replaced by typed host records without
//! changing the public SDK Message enum or waiting for a major SDK release.

use rho_sdk::model::Message;
use serde::{Deserialize, Serialize};

use rho_protocol::{display::NotificationCard, CompactionCounts};

const PREFIX: &str = "[rho boundary transcript v1]";
const FAMILY: &str = "[rho boundary transcript ";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum DisplayRow {
    /// Saved as `Message`; the name predates non-agent notifications.
    #[serde(rename = "Message")]
    Notification(Box<NotificationCard>),
    Notice(String),
    /// A compaction of the session's context, where it happened, with its
    /// context before and after. A load projects it from the compaction's
    /// node; it is never saved, so no saved record holds it.
    Compaction(CompactionCounts),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DisplayTranscript(pub(crate) Vec<DisplayRow>);

impl DisplayTranscript {
    pub(crate) fn display_message(&self) -> Message {
        Message::System(format!(
            "{PREFIX}\n{}",
            serde_json::to_string(self).expect("display rows contain only serializable data")
        ))
    }

    /// Unknown or damaged display records stay visible as a recoverable notice.
    /// Ordinary system text is not interpreted as host display metadata.
    pub(crate) fn from_display(text: &str) -> Option<Self> {
        if !text.starts_with(FAMILY) {
            return None;
        }
        // JSON accepts both LF and CRLF whitespace after the version marker.
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
                DisplayRow::Notification(card) => {
                    format!("{}\n{}\n{}", card.title, subtitle(card), card.plain_body())
                }
                DisplayRow::Notice(text) => text.clone(),
                DisplayRow::Compaction(counts) => {
                    format!(
                        "{COMPACTION_TITLE}\n{}",
                        compaction_lines(counts).join("\n")
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

impl DisplayRow {
    /// Text for hosts that show each row as its own titled card.
    /// A notice is titled by its first line.
    pub(crate) fn card_text(&self) -> CardText<'_> {
        match self {
            Self::Notification(card) => {
                let subtitle = subtitle(card);
                CardText {
                    title: &card.title,
                    markdown: format!("{subtitle}\n{}", card.markdown_body()),
                    plain: format!("{subtitle}\n{}", card.plain_body()),
                }
            }
            Self::Notice(text) => match text.split_once('\n') {
                Some((title, rest)) => CardText {
                    title,
                    markdown: rest.to_owned(),
                    plain: rest.to_owned(),
                },
                None => CardText {
                    title: text,
                    markdown: String::new(),
                    plain: String::new(),
                },
            },
            Self::Compaction(counts) => {
                let lines = compaction_lines(counts);
                CardText {
                    title: COMPACTION_TITLE,
                    // Markdown joins single line breaks into one line.
                    markdown: lines.join("\n\n"),
                    plain: lines.join("\n"),
                }
            }
        }
    }
}

/// How a compaction row is titled where it shows as text.
pub(crate) const COMPACTION_TITLE: &str = "Compacted context";

/// A compaction's counts, a line each: messages, tokens, and its cost when
/// the session priced it.
fn compaction_lines(counts: &CompactionCounts) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{} → {} messages",
            counts.previous_messages, counts.current_messages
        ),
        format!(
            "{} → {} estimated tokens",
            counts.previous_tokens, counts.current_tokens
        ),
    ];
    lines.extend(
        counts
            .cost_usd_micros
            .map(|micros| format!("${:.6} compaction cost", micros as f64 / 1_000_000.0)),
    );
    lines
}

/// One row's card title with its body in both host forms.
pub(crate) struct CardText<'a> {
    pub(crate) title: &'a str,
    /// For hosts that render markdown.
    pub(crate) markdown: String,
    /// For hosts that show text as is.
    pub(crate) plain: String,
}

/// The card's subtitle, or its routing line when it has none.
fn subtitle(card: &NotificationCard) -> String {
    card.subtitle.clone().unwrap_or_else(|| {
        let reference = card
            .reference
            .as_deref()
            .map(|id| format!(" · {id}"))
            .unwrap_or_default();
        format!("{} → {}{reference}", card.sender, card.recipient)
    })
}

#[cfg(test)]
#[path = "display_transcript_tests.rs"]
mod tests;
