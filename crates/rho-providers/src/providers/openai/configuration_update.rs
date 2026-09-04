//! Mid-conversation reasoning updates for OpenAI Responses.
//!
//! `gpt-6-astra` can change `reasoning.effort` between turns without busting
//! the prompt-cache prefix: leave request-level effort at the prefix baseline
//! and insert `configuration_update` items in `input`. Other models keep
//! today's request-level effort and never emit those items.
//!
//! Recorded effort lives on assistant messages as provider context so resume
//! and `/responses/compact` replacement history can recompute the same prefix.
//! Non-astra turns record nothing; later astra turns fall back to the current
//! request effort when no record exists.

use serde_json::{json, Value};

use crate::model::{Message, ModelError, ModelEvent, ModelIdentity, ProviderContextBlock};
use crate::protocol::openai_responses::lower_codex_history_message;

/// Provider-context kind for the effort that was in force for an assistant turn.
pub(super) const OPENAI_REASONING_EFFORT_KIND: &str = "openai_reasoning_effort";

const CONFIGURATION_UPDATE_MODEL: &str = "gpt-6-astra";
const CONFIGURATION_UPDATE_TYPE: &str = "configuration_update";

/// How create/compact bodies treat mid-conversation effort changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReasoningUpdatePolicy {
    /// Request-level effort is this turn's level. Never emit `configuration_update`.
    ///
    /// Compact uses this because `POST /responses/compact` rejects those items.
    CurrentLevel,
    /// Freeze request-level effort at the cached-prefix baseline on
    /// [`CONFIGURATION_UPDATE_MODEL`] and emit updates in `input`.
    PreservePrefix,
}

pub(super) fn preserves_prefix_for(model: &str) -> bool {
    model == CONFIGURATION_UPDATE_MODEL
}

pub(super) fn reasoning_effort_context(
    identity: &ModelIdentity,
    effort: &str,
) -> Option<ProviderContextBlock> {
    (preserves_prefix_for(&identity.model) && !effort.is_empty()).then(|| ProviderContextBlock {
        identity: identity.clone(),
        kind: OPENAI_REASONING_EFFORT_KIND.into(),
        position: None,
        data: Value::String(effort.to_owned()),
    })
}

/// Effort recorded on this assistant turn when it is replayable to `identity`.
fn recorded_effort<'a>(message: &'a Message, identity: &ModelIdentity) -> Option<&'a str> {
    assistant_context(message).and_then(|blocks| {
        blocks
            .iter()
            .find_map(|block| effort_from_block(block, identity))
    })
}

fn assistant_context(message: &Message) -> Option<&[ProviderContextBlock]> {
    match message {
        Message::EnrichedAssistant(message) => Some(&message.provider_context),
        Message::AbortedAssistant(message) => Some(&message.provider_context),
        Message::System(_) | Message::User(_) | Message::Assistant(_) | Message::ToolResult(_) => {
            None
        }
    }
}

fn effort_from_block<'a>(
    block: &'a ProviderContextBlock,
    identity: &ModelIdentity,
) -> Option<&'a str> {
    (block.kind == OPENAI_REASONING_EFFORT_KIND && block.is_replayable_to(identity))
        .then(|| block.data.as_str())
        .flatten()
        .filter(|effort| !effort.is_empty())
}

fn is_assistant_turn(message: &Message) -> bool {
    matches!(
        message,
        Message::Assistant(_) | Message::EnrichedAssistant(_) | Message::AbortedAssistant(_)
    )
}

fn configuration_update_item(effort: &str) -> Value {
    json!({
        "type": CONFIGURATION_UPDATE_TYPE,
        "reasoning": { "effort": effort },
    })
}

fn is_configuration_update(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some(CONFIGURATION_UPDATE_TYPE)
}

/// First recorded replayable effort, else the current turn's effort.
fn baseline_effort<'a>(
    messages: &'a [Message],
    identity: &ModelIdentity,
    current_effort: Option<&'a str>,
) -> Option<&'a str> {
    messages
        .iter()
        .find_map(|message| recorded_effort(message, identity))
        .or(current_effort)
}

/// History lowered for a Responses request, plus the two effort values.
///
/// `request_effort` stays on the create/compact body (prefix baseline when
/// preserving). `in_force_effort` is what the upcoming assistant turn will
/// actually run at after any inserted `configuration_update` items. When the
/// request ends on an assistant message there is no trailing segment to host
/// an update, so in-force stays at the last applied effort even if the user
/// asked for a new level.
pub(super) struct LoweredResponsesInput {
    pub(super) input: Vec<Value>,
    pub(super) request_effort: Option<String>,
    pub(super) in_force_effort: Option<String>,
}

/// Lowers history and, when preserving the prefix, inserts effort updates.
///
/// Each `configuration_update` is emitted immediately before the first input
/// item of the segment whose next assistant turn uses a different effort.
/// OpenAI documents placement before the next user message; placing it before
/// a `function_call_output` batch is our extrapolation so a `/reasoning`
/// change mid tool-loop still applies to the upcoming assistant turn.
///
/// Because an update is only inserted when that segment already has at least
/// one real item, two updates can never be adjacent and an update can never
/// be the last input item.
pub(super) fn lower_responses_input(
    messages: &[Message],
    instructions: &mut Vec<String>,
    identity: &ModelIdentity,
    current_effort: Option<&str>,
    policy: ReasoningUpdatePolicy,
) -> Result<LoweredResponsesInput, ModelError> {
    if policy == ReasoningUpdatePolicy::CurrentLevel || !preserves_prefix_for(&identity.model) {
        let input = crate::protocol::openai_responses::codex_input_items_for_target(
            messages,
            instructions,
            Some(identity),
        )?;
        let effort = current_effort.map(str::to_owned);
        return Ok(LoweredResponsesInput {
            input,
            request_effort: effort.clone(),
            in_force_effort: effort,
        });
    }

    let baseline = baseline_effort(messages, identity, current_effort);
    let (input, in_force_effort) = interleave_configuration_updates(
        messages,
        instructions,
        identity,
        baseline,
        current_effort,
    )?;
    debug_assert!(
        !has_adjacent_configuration_updates(&input),
        "configuration_update items must be followed by a real input item"
    );
    Ok(LoweredResponsesInput {
        input,
        request_effort: baseline.map(str::to_owned),
        in_force_effort,
    })
}

fn interleave_configuration_updates(
    messages: &[Message],
    instructions: &mut Vec<String>,
    identity: &ModelIdentity,
    baseline: Option<&str>,
    trailing_effort: Option<&str>,
) -> Result<(Vec<Value>, Option<String>), ModelError> {
    let mut input = Vec::new();
    let mut current_effort = baseline;
    let mut index = 0;
    while index < messages.len() {
        if is_assistant_turn(&messages[index]) {
            input.extend(lower_codex_history_message(
                &messages[index],
                instructions,
                Some(identity),
            )?);
            index += 1;
            continue;
        }

        let segment_start = index;
        while index < messages.len() && !is_assistant_turn(&messages[index]) {
            index += 1;
        }
        let desired_effort = if index < messages.len() {
            recorded_effort(&messages[index], identity).or(current_effort)
        } else {
            trailing_effort
        };

        let mut segment = Vec::new();
        for message in &messages[segment_start..index] {
            segment.extend(lower_codex_history_message(
                message,
                instructions,
                Some(identity),
            )?);
        }
        if let Some(desired) =
            desired_effort.filter(|desired| current_effort != Some(*desired) && !segment.is_empty())
        {
            input.push(configuration_update_item(desired));
            current_effort = Some(desired);
        }
        input.append(&mut segment);
    }
    Ok((input, current_effort.map(str::to_owned)))
}

pub(super) fn has_adjacent_configuration_updates(input: &[Value]) -> bool {
    input
        .windows(2)
        .any(|pair| is_configuration_update(&pair[0]) && is_configuration_update(&pair[1]))
}

#[cfg(test)]
pub(super) fn is_configuration_update_item(item: &Value) -> bool {
    is_configuration_update(item)
}

fn event_is_retained(event: &ModelEvent) -> bool {
    matches!(
        event,
        ModelEvent::OutputDelta(_)
            | ModelEvent::ReasoningSummaryDelta(_)
            | ModelEvent::ToolCallDelta { .. }
            | ModelEvent::ProviderContext { .. }
    )
}

/// Forwards one stream event, recording the in-force effort before the first
/// event that can survive in completed or aborted assistant history.
pub(super) fn forward_with_reasoning_effort(
    event: ModelEvent,
    effort: Option<&str>,
    emitted: &mut bool,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
) -> Result<(), ModelError> {
    if !*emitted && event_is_retained(&event) {
        emit_reasoning_effort(effort, on_event)?;
        *emitted = true;
    }
    if let Some(on_event) = on_event.as_mut() {
        on_event(event)
    } else {
        Ok(())
    }
}

/// Emits the in-force effort for a completed Responses turn.
///
/// Only emits when an effort was actually sent. Orchestration stores this on
/// the assistant message; later create-body lowering reads it back from history.
pub(super) fn emit_reasoning_effort(
    effort: Option<&str>,
    on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
) -> Result<(), ModelError> {
    let Some(effort) = effort.filter(|effort| !effort.is_empty()) else {
        return Ok(());
    };
    let Some(on_event) = on_event.as_mut() else {
        return Ok(());
    };
    on_event(ModelEvent::ProviderContext {
        kind: OPENAI_REASONING_EFFORT_KIND.into(),
        position: None,
        data: Value::String(effort.to_owned()),
    })
}

#[cfg(test)]
#[path = "configuration_update_tests.rs"]
mod tests;
