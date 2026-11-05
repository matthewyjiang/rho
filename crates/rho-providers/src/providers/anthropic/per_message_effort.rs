//! Anthropic per-message effort, rebuilt from history reasoning shifts.
//!
//! Models that support it keep top-level `output_config.effort` at the level
//! the conversation started with and send an effort-only `role: system`
//! message where each [`rho_sdk::model::ReasoningShift`] sits in history, so a
//! mid-session level change does not rewrite the cached prefix. Anthropic
//! requires those markers to be re-sent verbatim; history carries them across
//! resume, provider rebuilds, and providers shared between sessions.

use crate::{
    model::provider_models,
    protocol::anthropic_messages::{
        AnthropicContentBlock, AnthropicMessage, AnthropicOutputConfig, AnthropicRole,
        PlacedReasoningShift,
    },
    reasoning::ReasoningLevel,
};

/// Beta required for `output_config` on a `role: system` message.
pub(super) const BETA: &str = "mid-conversation-output-config-2026-07-01";

/// Inserts effort markers for `shifts` into `messages` and returns the
/// top-level `output_config` to send.
///
/// `current` is this request's config, and `effort_for` maps a reasoning level
/// to the effort the model sends for it (`None` when it sends none). Returns
/// `current` without markers when the model lacks per-message effort, a level
/// on the way has no effort, or history's latest level is not this request's
/// (for example classifier stages that run at another level): the pre-existing
/// top-level change.
pub(super) fn apply(
    model: &str,
    current: Option<AnthropicOutputConfig>,
    shifts: &[PlacedReasoningShift],
    effort_for: impl Fn(ReasoningLevel) -> Option<&'static str>,
    messages: &mut Vec<AnthropicMessage>,
) -> Option<AnthropicOutputConfig> {
    if !provider_models::supports_per_message_effort(model) {
        return current;
    }
    let Some(current_effort) = current.as_ref().map(|config| config.effort) else {
        return current;
    };
    // A shift that cannot take a marker restarts the prefix at its level, as
    // a new conversation would: the marker would split a tool_use/tool_result
    // pair, or take effect only after the upcoming response. The decision
    // depends only on history, so later requests rebuild the same prefix.
    let restart = shifts
        .iter()
        .rposition(|placed| !accepts_marker(messages, placed.at));
    let (prefix_level, placed) = match restart {
        Some(index) => (shifts[index].shift.to, &shifts[index + 1..]),
        None => match shifts.first() {
            Some(first) => (first.shift.from, shifts),
            None => return current,
        },
    };
    let Some(prefix_effort) = effort_for(prefix_level) else {
        return current;
    };
    let mut effort = prefix_effort;
    let mut markers = Vec::new();
    for placed in placed {
        let Some(next) = effort_for(placed.shift.to) else {
            return current;
        };
        if next != effort {
            markers.push((placed.at, next));
            effort = next;
        }
    }
    if effort != current_effort {
        return current;
    }
    for (at, effort) in markers.into_iter().rev() {
        messages.insert(at, AnthropicMessage::effort_change(effort));
    }
    Some(AnthropicOutputConfig {
        effort: prefix_effort,
    })
}

pub(super) fn beta_header(messages: &[AnthropicMessage]) -> Option<&'static str> {
    messages
        .iter()
        .any(|message| message.output_config.is_some())
        .then_some(BETA)
}

/// Whether a marker may go before `messages[at]`: it must be a user turn, and
/// not one carrying tool results, which must stay adjacent to the assistant
/// `tool_use` they answer. The new level applies from that turn.
fn accepts_marker(messages: &[AnthropicMessage], at: usize) -> bool {
    messages.get(at).is_some_and(|message| {
        message.role == AnthropicRole::User
            && !message
                .content
                .iter()
                .any(|block| matches!(block, AnthropicContentBlock::ToolResult { .. }))
    })
}

#[cfg(test)]
#[path = "per_message_effort_tests.rs"]
mod tests;
