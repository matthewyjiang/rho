//! Assistant content-block emission for complete envelopes and open snapshots.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::run_artifacts::AttachmentEvent;

use super::presentation::{
    content_block_kind, fidelity_notice, mark_and_reasoning, mark_and_text, mark_complete_index,
    mark_slot_emitted, push_block_slot, reasoning_effects, reconcile_complete_block,
    set_slot_tool_id, text_effects, tool_started_effects, tool_updated_effects, ContentBlockKind,
};
use super::tool_cards::StartedClaudeTool;
use super::types::StreamEffect;
use super::MessageStreamState;

/// Reconcile one block from a progressive assistant snapshot while the partial
/// stream is still open. Snapshot-local indices are not used.
pub(super) fn emit_open_snapshot_block(
    state: &mut MessageStreamState,
    block: &Value,
    active_tools: &mut HashMap<String, StartedClaudeTool>,
    max_active_tools: usize,
    cwd: Option<&Path>,
) -> Vec<StreamEffect> {
    let kind = content_block_kind(block.get("type").and_then(Value::as_str).unwrap_or(""));
    match kind {
        ContentBlockKind::Tool => {
            let tool_id = block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !tool_id.is_empty() && active_tools.contains_key(&tool_id) {
                return refresh_started_tool(active_tools, &tool_id, block, cwd);
            }
            // Prefer an existing unemitted tool slot; otherwise allocate.
            let ordinal = state
                .block_slots
                .iter()
                .position(|slot| slot.kind == ContentBlockKind::Tool && !slot.emitted)
                .or_else(|| push_block_slot(state, ContentBlockKind::Tool, None));
            let Some(ordinal) = ordinal else {
                return fidelity_notice(
                    "claude stream: dropped snapshot tool block; tracked block cap reached",
                );
            };
            let index = state.block_slots[ordinal].index;
            if !mark_slot_emitted(state, ordinal, index) {
                return fidelity_notice(
                    "claude stream: dropped snapshot tool block; tracked block cap reached",
                );
            }
            start_tool_effects(
                state,
                ordinal,
                active_tools,
                max_active_tools,
                &tool_id,
                block,
                cwd,
            )
        }
        ContentBlockKind::Text => emit_open_snapshot_text_like(
            state,
            ContentBlockKind::Text,
            block.get("text").and_then(Value::as_str),
            text_effects,
            "text",
        ),
        ContentBlockKind::Reasoning => {
            let text = block
                .get("thinking")
                .or_else(|| block.get("text"))
                .and_then(Value::as_str);
            emit_open_snapshot_text_like(
                state,
                ContentBlockKind::Reasoning,
                text,
                reasoning_effects,
                "reasoning",
            )
        }
        ContentBlockKind::Other => {
            let other = block.get("type").and_then(Value::as_str).unwrap_or("");
            if other.is_empty() {
                Vec::new()
            } else {
                vec![StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                    "claude stream: ignored assistant block `{other}`"
                )))]
            }
        }
    }
}

/// Shared Text/Reasoning snapshot path: skip when already emitted, else claim
/// an unemitted same-kind slot (or allocate) and present the snapshot body.
fn emit_open_snapshot_text_like(
    state: &mut MessageStreamState,
    kind: ContentBlockKind,
    text: Option<&str>,
    present: fn(&str) -> Vec<StreamEffect>,
    label: &str,
) -> Vec<StreamEffect> {
    let Some(text) = text else {
        return Vec::new();
    };
    if text.is_empty() {
        return Vec::new();
    }
    if state
        .block_slots
        .iter()
        .any(|slot| slot.kind == kind && slot.emitted)
    {
        return Vec::new();
    }
    let ordinal = state
        .block_slots
        .iter()
        .position(|slot| slot.kind == kind && !slot.emitted)
        .or_else(|| push_block_slot(state, kind, None));
    let Some(ordinal) = ordinal else {
        return fidelity_notice(&format!(
            "claude stream: dropped snapshot {label} block; tracked block cap reached"
        ));
    };
    let index = state.block_slots[ordinal].index.unwrap_or(ordinal);
    if !mark_slot_emitted(state, ordinal, Some(index)) {
        return fidelity_notice(&format!(
            "claude stream: dropped snapshot {label} block; tracked block cap reached"
        ));
    }
    present(text)
}

pub(super) fn emit_complete_block(
    state: &mut MessageStreamState,
    block: &Value,
    index: usize,
    active_tools: &mut HashMap<String, StartedClaudeTool>,
    max_active_tools: usize,
    cwd: Option<&Path>,
) -> Vec<StreamEffect> {
    let kind = content_block_kind(block.get("type").and_then(Value::as_str).unwrap_or(""));
    match kind {
        ContentBlockKind::Text => {
            let Some(text) = block.get("text").and_then(Value::as_str) else {
                return Vec::new();
            };
            mark_and_text(state, index, text).unwrap_or_else(|| {
                fidelity_notice(
                    "claude stream: dropped complete text block; tracked block cap reached",
                )
            })
        }
        ContentBlockKind::Reasoning => {
            let Some(text) = block
                .get("thinking")
                .or_else(|| block.get("text"))
                .and_then(Value::as_str)
            else {
                return Vec::new();
            };
            mark_and_reasoning(state, index, text).unwrap_or_else(|| {
                fidelity_notice(
                    "claude stream: dropped complete reasoning block; tracked block cap reached",
                )
            })
        }
        ContentBlockKind::Tool => {
            let tool_id = block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let already_started = !tool_id.is_empty() && active_tools.contains_key(&tool_id);
            let already_emitted = reconcile_complete_block(state, ContentBlockKind::Tool, index);
            if already_started || already_emitted {
                // Started via partials or already presented; still mark this
                // complete index seen. Never restart a finished or evicted id.
                let _ = mark_complete_index(state, index, ContentBlockKind::Tool);
                return refresh_started_tool(active_tools, &tool_id, block, cwd);
            }
            if !mark_complete_index(state, index, ContentBlockKind::Tool) {
                return fidelity_notice(
                    "claude stream: dropped complete tool block; tracked block cap reached",
                );
            }
            let Some(ordinal) = state
                .block_slots
                .iter()
                .position(|slot| slot.index == Some(index))
            else {
                return fidelity_notice(
                    "claude stream: dropped complete tool block; tracked block cap reached",
                );
            };
            start_tool_effects(
                state,
                ordinal,
                active_tools,
                max_active_tools,
                &tool_id,
                block,
                cwd,
            )
        }
        ContentBlockKind::Other => {
            let other = block.get("type").and_then(Value::as_str).unwrap_or("");
            if other.is_empty() {
                Vec::new()
            } else {
                vec![StreamEffect::Attachment(AttachmentEvent::Notice(format!(
                    "claude stream: ignored assistant block `{other}`"
                )))]
            }
        }
    }
}

pub(super) fn note_tool_started(
    active_tools: &mut HashMap<String, StartedClaudeTool>,
    max_active_tools: usize,
    tool_id: &str,
    tool: StartedClaudeTool,
) -> Option<Vec<StreamEffect>> {
    if tool_id.is_empty() || active_tools.contains_key(tool_id) {
        return None;
    }
    if active_tools.len() >= max_active_tools {
        // Drop an arbitrary active id so pathological streams stay bounded.
        if let Some(old) = active_tools.keys().next().cloned() {
            active_tools.remove(&old);
            active_tools.insert(tool_id.to_string(), tool);
            return Some(fidelity_notice(
                "claude stream: evicted active tool id; tool finish pairing may be imperfect",
            ));
        }
    }
    active_tools.insert(tool_id.to_string(), tool);
    None
}

fn start_tool_effects(
    state: &mut MessageStreamState,
    ordinal: usize,
    active_tools: &mut HashMap<String, StartedClaudeTool>,
    max_active_tools: usize,
    tool_id: &str,
    block: &Value,
    cwd: Option<&Path>,
) -> Vec<StreamEffect> {
    set_slot_tool_id(state, ordinal, tool_id);
    let tool = StartedClaudeTool::from_block(block);
    let mut effects = Vec::new();
    if let Some(notice) = note_tool_started(active_tools, max_active_tools, tool_id, tool.clone()) {
        effects.extend(notice);
    }
    effects.extend(tool_started_effects(tool_id, &tool, cwd));
    effects
}

pub(super) fn refresh_started_tool(
    active_tools: &mut HashMap<String, StartedClaudeTool>,
    tool_id: &str,
    block: &Value,
    cwd: Option<&Path>,
) -> Vec<StreamEffect> {
    let updated = active_tools
        .get_mut(tool_id)
        .and_then(|tool| tool.apply_input(block.get("input")).then(|| tool.clone()));
    updated
        .map(|tool| tool_updated_effects(tool_id, &tool, cwd))
        .unwrap_or_default()
}
