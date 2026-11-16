//! Replay of interrupted assistant turns, shared by every wire.

use crate::model::{AbortedAssistant, AssistantMessage, ContentBlock};

/// The assistant message a provider replays for an aborted turn, from
/// `content` the provider has already lowered (for example with partial tool
/// calls rendered as non-executable text).
///
/// The reasoning summary never travels, so `handoff::prepare_assistant`
/// cannot fall back to sending it as text. It is the model's own partial
/// reasoning in readable prose, and providers refuse requests that hand a
/// model its reasoning back that way (Anthropic: `reasoning_extraction`).
/// Reasoning crosses an abort only as opaque `provider_context`.
pub(crate) fn aborted_replay_message(
    message: &AbortedAssistant,
    content: Vec<ContentBlock>,
) -> AssistantMessage {
    AssistantMessage {
        content,
        provenance: message.provenance.clone(),
        reasoning_summary: None,
        provider_context: message.provider_context.clone(),
    }
}

#[cfg(test)]
#[path = "aborted_replay_tests.rs"]
mod tests;
