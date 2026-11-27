//! Display evidence only: snapshots, provider envelopes and token accounting
//! never enter the retrieval index. Evidence is the display rows a load of a
//! session replays, along its active branch, so a match is always one a
//! client that opens the session can see. Anchors bind a saved row to its
//! evidence.

use std::collections::HashSet;

use rho_protocol::SearchLocation;
use rho_providers::model::{ContentBlock, Message};
use sha2::{Digest, Sha256};

use super::{search::TOOL_NAME, tree::ProjectedMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Evidence {
    pub anchor: String,
    pub role: &'static str,
    pub text: String,
    pub omitted_blocks: usize,
    /// Where an attach's replay shows the row.
    pub at: Option<SearchLocation>,
    /// When the row was saved, in unix seconds.
    pub saved_at: Option<u64>,
}

impl Evidence {
    /// Appends preserve anchors; replacement evidence at the same row does
    /// not. Hash field boundaries explicitly, including a fixed-width count.
    fn anchor(position: String, role: &str, text: &str, omitted_blocks: usize) -> String {
        let mut digest = Sha256::new();
        digest.update(role.as_bytes());
        digest.update([0]);
        digest.update(text.as_bytes());
        digest.update((omitted_blocks as u64).to_le_bytes());
        format!("{position}:{}", hex::encode(digest.finalize()))
    }
}

/// The evidence of `rows`, a session's display rows as a load replays them,
/// each with where replay shows it: `locations`, one per row. Rows the load
/// added, which no saved row holds, are left out. The sessions tool's calls
/// and results only echo queries and other evidence, so they are dropped
/// too.
pub(super) fn extract(
    rows: &[ProjectedMessage],
    locations: Vec<Option<SearchLocation>>,
) -> Vec<Evidence> {
    debug_assert_eq!(rows.len(), locations.len());
    let mut sessions_calls = HashSet::new();
    rows.iter()
        .zip(locations)
        .filter_map(|(row, at)| {
            let origin = row.origin.as_ref()?;
            let (role, text, omitted_blocks) = row_text(&row.message, &mut sessions_calls)?;
            Some(Evidence {
                anchor: Evidence::anchor(
                    format!("{}:{}", origin.node, origin.row),
                    role,
                    &text,
                    omitted_blocks,
                ),
                role,
                text,
                omitted_blocks,
                at,
                saved_at: row.saved_at.map(|millis| millis / 1000),
            })
        })
        .collect()
}

/// A message's role, searchable text and how many blocks it has that are
/// not text, or `None` when it holds no evidence. Remembers the sessions
/// tool's call ids in `sessions_calls` to drop their later results.
fn row_text(
    message: &Message,
    sessions_calls: &mut HashSet<String>,
) -> Option<(&'static str, String, usize)> {
    let (role, blocks, partial_calls) = match message {
        Message::ToolResult(result) => {
            if sessions_calls.contains(&result.id) {
                return None;
            }
            let role = if result.is_failure() {
                "tool_error"
            } else {
                "tool_result"
            };
            return Some((role, result.content.clone(), 0));
        }
        Message::User(blocks) | Message::BoundaryInput(blocks) => ("user", &blocks[..], &[][..]),
        Message::Assistant(blocks) => ("assistant", &blocks[..], &[][..]),
        Message::EnrichedAssistant(message) => ("assistant", &message.content[..], &[][..]),
        Message::AbortedAssistant(message) => (
            "assistant_aborted",
            &message.content[..],
            &message.tool_calls[..],
        ),
        // Display rows of these hold no conversation text: cards, notices
        // and model-only entries.
        Message::System(_)
        | Message::CompactionSummary(_)
        | Message::ModelContext(_)
        | Message::ReasoningShift(_) => return None,
    };
    let mut parts = Vec::new();
    let mut omitted_blocks = 0;
    for block in blocks {
        match block {
            ContentBlock::Text(text) => parts.push(text.clone()),
            ContentBlock::ToolCall(call) if call.name == TOOL_NAME => {
                sessions_calls.insert(call.id.clone());
            }
            ContentBlock::ToolCall(call) => {
                parts.push(format!("tool_call {} {}", call.name, call.arguments));
            }
            // Image bytes are not textual evidence. Reads report omissions.
            ContentBlock::Image(_) => omitted_blocks += 1,
        }
    }
    for call in partial_calls
        .iter()
        .filter(|call| call.name.as_deref() != Some(TOOL_NAME))
    {
        parts.push(format!(
            "partial_tool_call {} {}",
            call.name.as_deref().unwrap_or("unknown"),
            call.arguments
        ));
    }
    // A message holding only a `sessions` call has no evidence left.
    if parts.is_empty() && omitted_blocks == 0 {
        return None;
    }
    Some((role, parts.join("\n"), omitted_blocks))
}
