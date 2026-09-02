use std::collections::VecDeque;

use crate::{
    model::provider_models,
    protocol::anthropic_messages::{
        AnthropicContentBlock, AnthropicMessage, AnthropicOutputConfig, AnthropicRole,
    },
};

/// Beta required for `output_config` on a `role: system` message.
pub(super) const BETA: &str = "mid-conversation-output-config-2026-07-01";

/// Retained conversations per provider instance. Nothing tells the provider
/// when a session ends, so this is an LRU cap rather than lifecycle cleanup.
/// An entry is one owned key plus a few `&'static str` shifts, so the cap is
/// sized as a tripwire, not a budget: no host runs anywhere near this many
/// live Anthropic sessions on one provider. Evicting a live conversation only
/// falls back to a top-level effort change, which is the pre-existing path.
pub(super) const MAX_TRACKED_CONVERSATIONS: usize = 1_024;

/// Per-conversation effort state on one Anthropic provider instance.
///
/// A provider can be shared across sessions (`RhoBuilder::provider_shared`),
/// so state is keyed by the request's `prompt_cache_key`. That key is the
/// session identity Rho already derives for cache continuity. Requests
/// without one get today's top-level effort and never touch this list.
/// Most recently used entries sit at the back; the front is evicted past
/// [`MAX_TRACKED_CONVERSATIONS`].
///
/// NEXT_MAJOR(rho-sdk): record reasoning shifts as a first-class history
/// message appended by `set_reasoning_level`, and have the Anthropic
/// converter emit the empty `role: system` message from history. Then delete
/// this provider-local state. Anthropic requires the marker to be re-sent
/// verbatim; rebuilding it from memory loses it on resume or provider rebuild,
/// which falls back to a top-level effort change and busts the cache.
#[derive(Debug, Default)]
pub(super) struct PerMessageEffortState {
    conversations: VecDeque<(String, ConversationEffort)>,
}

impl PerMessageEffortState {
    #[cfg(test)]
    pub(super) fn tracked(&self) -> usize {
        self.conversations.len()
    }

    #[cfg(test)]
    pub(super) fn is_tracked(&self, conversation: &str) -> bool {
        self.conversations
            .iter()
            .any(|(key, _)| key == conversation)
    }

    fn remove(&mut self, conversation: &str) -> Option<ConversationEffort> {
        let index = self
            .conversations
            .iter()
            .position(|(key, _)| key == conversation)?;
        self.conversations.remove(index).map(|(_, entry)| entry)
    }

    /// Moves `conversation` to the most-recent slot and returns it, or
    /// returns `None` if it is not tracked.
    fn touch(&mut self, conversation: &str) -> Option<&mut ConversationEffort> {
        let entry = self.remove(conversation)?;
        self.conversations
            .push_back((conversation.to_owned(), entry));
        self.conversations.back_mut().map(|(_, entry)| entry)
    }

    fn insert(&mut self, conversation: &str, entry: ConversationEffort) {
        self.remove(conversation);
        if self.conversations.len() >= MAX_TRACKED_CONVERSATIONS {
            self.conversations.pop_front();
        }
        self.conversations
            .push_back((conversation.to_owned(), entry));
    }
}

/// Prefix effort and in-history shifts for one conversation.
///
/// Consecutive requests keep top-level `output_config.effort` on the first
/// value and append effort-only system messages at the change points so the
/// cached prefix still matches.
#[derive(Clone, Debug)]
struct ConversationEffort {
    prefix_effort: &'static str,
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
    conversation: Option<&str>,
    current: Option<AnthropicOutputConfig>,
    messages: &mut Vec<AnthropicMessage>,
) -> Option<AnthropicOutputConfig> {
    if !provider_models::supports_per_message_effort(model) {
        return current;
    }
    let Some(conversation) = conversation else {
        return current;
    };
    let Some(current_effort) = current.as_ref().map(|config| config.effort) else {
        state.remove(conversation);
        return current;
    };

    let uninjected_len = messages.len();
    let fresh = ConversationEffort {
        prefix_effort: current_effort,
        shifts: Vec::new(),
        last_uninjected_len: uninjected_len,
    };
    let Some(entry) = state.touch(conversation) else {
        state.insert(conversation, fresh);
        return current;
    };

    if uninjected_len < entry.last_uninjected_len {
        *entry = fresh;
        return current;
    }

    let last_effort = entry
        .shifts
        .last()
        .map(|shift| shift.effort)
        .unwrap_or(entry.prefix_effort);
    if current_effort != last_effort {
        if uninjected_len == entry.last_uninjected_len {
            // Same prompt, different effort: classifier stages, not a
            // conversation continuation. Keep today's top-level change.
            *entry = fresh;
            return current;
        }
        entry.shifts.push(EffortShift {
            at: shift_insert_index(messages),
            effort: current_effort,
        });
    }

    insert_shifts(messages, &entry.shifts);
    entry.last_uninjected_len = uninjected_len;
    Some(AnthropicOutputConfig {
        effort: entry.prefix_effort,
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

/// Marker index for a new shift: before the latest user turn, unless that
/// turn carries tool results. A tool-result turn must stay adjacent to the
/// assistant `tool_use` it answers, so splitting the pair with a system
/// marker would 400 the request. The marker goes after the tool-result turn
/// instead; the new level then applies from the upcoming assistant response.
fn shift_insert_index(messages: &[AnthropicMessage]) -> usize {
    let at = last_user_index(messages);
    let ends_tool_pair = messages.get(at).is_some_and(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, AnthropicContentBlock::ToolResult { .. }))
    });
    if ends_tool_pair {
        at + 1
    } else {
        at
    }
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
