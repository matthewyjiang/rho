use crate::{
    model::provider_models,
    protocol::anthropic_messages::{AnthropicMessage, AnthropicOutputConfig, AnthropicRole},
};

/// Beta required for `output_config` on a `role: system` message.
pub(super) const BETA: &str = "mid-conversation-output-config-2026-07-01";

/// Prefix effort and in-history shifts for one Anthropic provider instance.
///
/// Consecutive conversation requests on the same instance keep top-level
/// `output_config.effort` on the first value and append effort-only system
/// messages at the change points so the cached prefix still matches.
///
/// NEXT_MAJOR(rho-sdk): record reasoning shifts as a first-class history
/// message appended by `set_reasoning_level`, and have the Anthropic
/// converter emit the empty `role: system` message from history. Then delete
/// this provider-local state. Anthropic requires the marker to be re-sent
/// verbatim; rebuilding it from memory loses it on resume or provider rebuild,
/// which falls back to a top-level effort change and busts the cache.
#[derive(Clone, Debug, Default)]
pub(super) struct PerMessageEffortState {
    prefix_effort: Option<&'static str>,
    shifts: Vec<EffortShift>,
    last_uninjected_len: usize,
}

#[derive(Clone, Copy, Debug)]
struct EffortShift {
    at: usize,
    effort: &'static str,
}

/// Rewrites `messages` and returns the top-level output_config to send.
pub(super) fn apply(
    model: &str,
    state: &mut PerMessageEffortState,
    current: Option<AnthropicOutputConfig>,
    messages: &mut Vec<AnthropicMessage>,
) -> Option<AnthropicOutputConfig> {
    if !provider_models::supports_per_message_effort(model) {
        return current;
    }
    let Some(current_effort) = current.as_ref().map(|config| config.effort) else {
        *state = PerMessageEffortState::default();
        return current;
    };

    let uninjected_len = messages.len();
    let Some(prefix_effort) = state.prefix_effort else {
        state.prefix_effort = Some(current_effort);
        state.last_uninjected_len = uninjected_len;
        return current;
    };

    if uninjected_len < state.last_uninjected_len {
        *state = PerMessageEffortState {
            prefix_effort: Some(current_effort),
            last_uninjected_len: uninjected_len,
            shifts: Vec::new(),
        };
        return current;
    }

    let last_effort = state
        .shifts
        .last()
        .map(|shift| shift.effort)
        .unwrap_or(prefix_effort);
    if current_effort != last_effort {
        if uninjected_len == state.last_uninjected_len {
            // Same prompt, different effort: classifier stages, not a
            // conversation continuation. Keep today's top-level change.
            *state = PerMessageEffortState {
                prefix_effort: Some(current_effort),
                last_uninjected_len: uninjected_len,
                shifts: Vec::new(),
            };
            return current;
        }
        state.shifts.push(EffortShift {
            at: last_user_index(messages),
            effort: current_effort,
        });
    }

    insert_shifts(messages, &state.shifts);
    state.last_uninjected_len = uninjected_len;
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

fn last_user_index(messages: &[AnthropicMessage]) -> usize {
    messages
        .iter()
        .rposition(|message| message.role == AnthropicRole::User)
        .unwrap_or(messages.len())
}

fn insert_shifts(messages: &mut Vec<AnthropicMessage>, shifts: &[EffortShift]) {
    for shift in shifts.iter().rev() {
        let at = shift.at.min(messages.len());
        messages.insert(at, AnthropicMessage::effort_change(shift.effort));
    }
}

#[cfg(test)]
#[path = "per_message_effort_tests.rs"]
mod tests;
