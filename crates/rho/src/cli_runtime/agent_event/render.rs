//! Stateful protocol-to-artifact translation; terminal decisions stay in the driver.

use std::{collections::HashMap, path::PathBuf};

use agent_client_protocol::schema::v1::{
    ContentBlock, Plan, PlanEntryStatus, SessionUpdate, StopReason, ToolCallId, UsageUpdate,
};
use rho_sdk::model::ContextUsage;
use rho_tools::tool_card::{ToolCard, ToolFamily, ToolHeader, ToolStatus};

use crate::{
    cli_runtime::{
        stream_effect::{StatusPatch, StreamEffect},
        stream_format::{bound_result_text, reasoning_effects, set_lines_body, text_effects},
    },
    run_artifacts::AttachmentEvent,
};

use super::{
    tool_cards::{content_notice, finished_card, started_card, StartedTool},
    AgentEvent,
};

/// Bound on tool calls retained while in flight. Carried over from the deleted
/// Cursor `-p` mapper: its spike peaked at 33 sequential calls, and parallel
/// fan-out inside one turn is the only way to exceed this. Overflow only loses
/// update enrichment and is announced with a notice.
const MAX_ACTIVE_TOOLS: usize = 256;

/// Renders one external-agent session, retaining only bounded tool snapshots
/// and final assistant text. The cwd is used for display paths, never I/O.
#[derive(Debug)]
pub(crate) struct EventRenderer {
    active_tools: HashMap<ToolCallId, StartedTool>,
    cwd: PathBuf,
    step_started: bool,
    plan_started: bool,
    result_text: String,
    /// A tool call or turn end closed the current assistant segment; the next
    /// text chunk starts a fresh result instead of appending to narration.
    result_segment_closed: bool,
}

impl EventRenderer {
    pub(crate) fn new(cwd: PathBuf) -> Self {
        Self {
            active_tools: HashMap::new(),
            cwd,
            step_started: false,
            plan_started: false,
            result_text: String::new(),
            result_segment_closed: false,
        }
    }

    /// Bounded final assistant text for the driver's result.json: the text after
    /// the last tool call or turn boundary, matching `-p` runtimes' `result`.
    /// Earlier segments are kept only until a newer segment produces text.
    pub(crate) fn result_text(&self) -> &str {
        &self.result_text
    }

    pub(crate) fn render(&mut self, event: AgentEvent) -> Vec<StreamEffect> {
        match event {
            AgentEvent::Update(update) => self.render_update(*update),
            AgentEvent::TurnEnded(reason) => {
                self.step_started = false;
                self.result_segment_closed = true;
                let label = match reason {
                    StopReason::EndTurn => "end_turn",
                    StopReason::MaxTokens => "max_tokens",
                    StopReason::MaxTurnRequests => "max_turn_requests",
                    StopReason::Refusal => "refusal",
                    StopReason::Cancelled => "cancelled",
                    _ => return notice("acp: unhandled stop reason".into()),
                };
                vec![StreamEffect::Status(StatusPatch {
                    last_activity: Some(format!("turn ended: {label}")),
                    ..StatusPatch::default()
                })]
            }
            AgentEvent::Notice(message) => notice(message),
        }
    }

    fn render_update(&mut self, update: SessionUpdate) -> Vec<StreamEffect> {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
                ContentBlock::Text(text) => {
                    if std::mem::take(&mut self.result_segment_closed) {
                        self.result_text.clear();
                    }
                    self.result_text.push_str(&text.text);
                    self.result_text = bound_result_text(&self.result_text);
                    self.with_step_started(text_effects(&text.text))
                }
                other => content_notice(&other).map_or_else(Vec::new, notice),
            },
            SessionUpdate::AgentThoughtChunk(chunk) => match chunk.content {
                ContentBlock::Text(text) => self.with_step_started(reasoning_effects(&text.text)),
                other => content_notice(&other).map_or_else(Vec::new, notice),
            },
            SessionUpdate::UserMessageChunk(_) => Vec::new(),
            SessionUpdate::ToolCall(call) => {
                self.result_segment_closed = true;
                let key = call.tool_call_id.clone();
                let (tool, notices) = started_card(&call, &self.cwd);
                let mut effects = notices.into_iter().flat_map(notice).collect::<Vec<_>>();
                effects.push(StreamEffect::Attachment(AttachmentEvent::ToolStarted {
                    key: Some(key.to_string()),
                    card: tool.card.clone().into(),
                }));
                // Terminal initial snapshots still render as ToolStarted, but
                // need no retained state unless an update follows.
                match tool.card.status {
                    ToolStatus::Running => {
                        if self.active_tools.contains_key(&key)
                            || self.has_tool_capacity(&mut effects)
                        {
                            self.active_tools.insert(key, tool);
                        }
                    }
                    ToolStatus::Ok | ToolStatus::Error | ToolStatus::Interrupted => {
                        self.active_tools.remove(&key);
                    }
                }
                self.with_step_started(effects)
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let key = update.tool_call_id.clone();
                let known = self.active_tools.contains_key(&key);
                let mut tool = self
                    .active_tools
                    .remove(&key)
                    .unwrap_or_else(StartedTool::unknown);
                let (card, notices) = finished_card(&mut tool, &update, &self.cwd);
                let mut effects = notices.into_iter().flat_map(notice).collect::<Vec<_>>();
                let finished = match card.status {
                    ToolStatus::Ok | ToolStatus::Error => true,
                    ToolStatus::Running | ToolStatus::Interrupted => false,
                };
                let event = if finished {
                    AttachmentEvent::ToolFinished {
                        key: Some(key.to_string()),
                        presentation: card.into(),
                    }
                } else {
                    if known || self.has_tool_capacity(&mut effects) {
                        self.active_tools.insert(key.clone(), tool);
                    }
                    AttachmentEvent::ToolUpdated {
                        key: Some(key.to_string()),
                        card: card.into(),
                    }
                };
                effects.push(StreamEffect::Attachment(event));
                self.with_step_started(effects)
            }
            SessionUpdate::Plan(plan) => self.render_plan(plan),
            SessionUpdate::CurrentModeUpdate(mode) => vec![StreamEffect::Status(StatusPatch {
                last_activity: Some(format!("mode: {}", mode.current_mode_id)),
                ..StatusPatch::default()
            })],
            SessionUpdate::UsageUpdate(usage) => self.render_usage(usage),
            SessionUpdate::AvailableCommandsUpdate(_)
            | SessionUpdate::ConfigOptionUpdate(_)
            | SessionUpdate::SessionInfoUpdate(_) => Vec::new(),
            _ => notice("acp: unhandled session update".into()),
        }
    }

    fn render_plan(&mut self, plan: Plan) -> Vec<StreamEffect> {
        let mut effects = Vec::new();
        let mut lines = Vec::new();
        for entry in plan.entries {
            let marker = match entry.status {
                PlanEntryStatus::Pending => "☐",
                PlanEntryStatus::InProgress => "◐",
                PlanEntryStatus::Completed => "☑",
                _ => {
                    effects.extend(notice("acp: unhandled plan entry status".into()));
                    "?"
                }
            };
            lines.push(format!("{marker} {}", entry.content));
        }
        let mut card = ToolCard::new(
            ToolStatus::Running,
            ToolFamily::Form,
            ToolHeader::call("Plan", /*primary*/ None),
        );
        set_lines_body(&mut card, &lines.join("\n"));
        let key = Some("acp-plan".into());
        let event = if self.plan_started {
            AttachmentEvent::ToolUpdated {
                key,
                card: card.into(),
            }
        } else {
            self.plan_started = true;
            AttachmentEvent::ToolStarted {
                key,
                card: card.into(),
            }
        };
        effects.push(StreamEffect::Attachment(event));
        self.with_step_started(effects)
    }

    fn render_usage(&self, usage: UsageUpdate) -> Vec<StreamEffect> {
        let mut effects = vec![StreamEffect::Attachment(AttachmentEvent::ContextUsage(
            ContextUsage::provider_reported(usage.used, Some(usage.size)),
        ))];
        if let Some(cost) = usage.cost {
            // Status can represent USD only; never silently relabel another currency.
            if !cost.currency.eq_ignore_ascii_case("USD") {
                effects.extend(notice(format!(
                    "acp: unsupported cost currency {}",
                    cost.currency
                )));
            } else if !cost.amount.is_finite() || cost.amount < 0.0 {
                effects.extend(notice("acp: invalid session cost".into()));
            } else {
                effects.push(StreamEffect::Status(StatusPatch {
                    total_cost_usd: Some(cost.amount),
                    ..StatusPatch::default()
                }));
            }
        }
        effects
    }

    /// Losing enrichment is fail-soft, but the budget must be visible.
    fn has_tool_capacity(&self, effects: &mut Vec<StreamEffect>) -> bool {
        let asked = self.active_tools.len() + 1;
        if asked <= MAX_ACTIVE_TOOLS {
            true
        } else {
            effects.extend(notice(format!(
                "acp: active tool budget {MAX_ACTIVE_TOOLS}, requested {asked}; card renders without retained update state"
            )));
            false
        }
    }

    fn with_step_started(&mut self, mut effects: Vec<StreamEffect>) -> Vec<StreamEffect> {
        if !self.step_started && !effects.is_empty() {
            self.step_started = true;
            effects.insert(0, StreamEffect::Attachment(AttachmentEvent::StepStarted));
        }
        effects
    }
}

fn notice(message: String) -> Vec<StreamEffect> {
    vec![StreamEffect::Attachment(AttachmentEvent::Notice(message))]
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
