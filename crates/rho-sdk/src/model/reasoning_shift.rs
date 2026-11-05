//! Reasoning-level changes recorded in conversation history.

use serde::{Deserialize, Serialize};

use super::Message;
use crate::ReasoningLevel;

/// The session's reasoning level changed before the next history entry.
///
/// [`crate::Session::set_reasoning_level`] appends this so providers that pin
/// effort mid-conversation (for example Anthropic per-message effort) can
/// rebuild the same request prefix from history on every turn, including after
/// resume or a provider rebuild. Providers without such a mechanism send
/// nothing for it; the request's [`super::ModelRequest::reasoning_level`] stays
/// the level for the upcoming turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningShift {
    pub from: ReasoningLevel,
    pub to: ReasoningLevel,
}

/// Records a change from `from` to `to` at the end of `history`.
///
/// Nothing is recorded before the conversation starts (only system entries),
/// since there is no prefix to preserve. Consecutive changes collapse into one
/// shift, and a change back to the level before them removes it. Returns
/// whether `history` changed.
pub(crate) fn record(history: &mut Vec<Message>, from: ReasoningLevel, to: ReasoningLevel) -> bool {
    if from == to {
        return false;
    }
    if let Some(Message::ReasoningShift(last)) = history.last_mut() {
        if last.from == to {
            history.pop();
        } else {
            last.to = to;
        }
        return true;
    }
    let started = history
        .iter()
        .any(|message| !matches!(message, Message::System(_)));
    if !started {
        return false;
    }
    history.push(Message::ReasoningShift(ReasoningShift { from, to }));
    true
}

#[cfg(test)]
#[path = "reasoning_shift_tests.rs"]
mod tests;
