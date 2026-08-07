//! Progress snapshots the advisor tool sends into its tool card.
//!
//! Each message is a complete display snapshot: a status line, then optional
//! guidance body. The presenter and tool share this codec so status text never
//! leaks into the final model-visible tool result.

use crate::agent::OneShotPhase;

/// Builds a progress message from a live one-shot update.
pub(crate) fn encode(phase: OneShotPhase, text: &str) -> String {
    let label = phase.label();
    if text.is_empty() {
        label.to_owned()
    } else {
        format!("{label}\n\n{text}")
    }
}

/// Splits a progress message into status detail and streamed body.
pub(crate) fn decode(message: &str) -> (&str, &str) {
    match message.split_once("\n\n") {
        Some((phase, body)) => (phase, body),
        None => (message, ""),
    }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
