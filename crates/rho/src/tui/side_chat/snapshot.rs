use rho_sdk::model::Message;

use crate::tools::advisor::{render_transcript, DEFAULT_TRANSCRIPT_BUDGET};

const SNAPSHOT_PREAMBLE: &str = "\
This is a frozen snapshot of the parent Rho session. It is background, not a \
question. The user's aside follows in later messages.\n\n";

/// Packs parent history into an owned snapshot taken at this instant.
pub(super) fn frozen_parent_snapshot(messages: &[Message]) -> String {
    let mut snapshot = String::from(SNAPSHOT_PREAMBLE);
    snapshot.push_str(&render_transcript(
        None,
        messages,
        DEFAULT_TRANSCRIPT_BUDGET,
    ));
    snapshot
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
