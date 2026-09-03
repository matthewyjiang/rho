//! Decode `cursor-agent` `stream-json` lines into Rho run artifacts.
//!
//! Emits the same [`StreamEffect`] vocabulary as the Claude mapper so the
//! session sink, drain loop, and terminal assessment are shared. Only the
//! wire protocol differs.
//!
//! # Assistant snapshots
//!
//! With `--stream-partial-output`, Cursor streams `assistant` text deltas and
//! then repeats the whole segment as one cumulative `assistant` frame:
//! mid-turn (right before a tool call, tagged `model_call_id`) and once more
//! at the very end (no `timestamp_ms`). Appending those would double-render.
//! [`CursorStreamMapper`] tracks the text accumulated since the last segment
//! boundary and drops a frame whose text equals it. A `tool_call/started`
//! also closes the segment. The rule is by content, not by key, so a future
//! build that changes the marker keys still renders once.
//!
//! # Terminal results
//!
//! `result` becomes [`StreamEffect::Terminal`] metadata only; the session
//! combines it with process exit, as for Claude. Cursor has no `error` frame:
//! startup failures surface on stderr with a nonzero exit.

mod protocol;
mod tool_cards;

use std::collections::HashMap;
use std::path::PathBuf;

use rho_sdk::model::ModelUsage;

use crate::claude_runtime::drain::StreamLineMapper;
#[cfg(test)]
use crate::claude_runtime::stream::apply_status_patch;
use crate::claude_runtime::stream::{
    bound_result_text, classify_terminal_result, reasoning_effects, text_effects, StatusPatch,
    StreamEffect, TerminalClassification, TerminalResult,
};
use crate::{run_artifacts::AttachmentEvent, subagent::RunState};

use protocol::{
    decode_frame, AssistantFrame, CursorFrame, InitFrame, ResultFrame, ToolCallFrame, ToolCallPhase,
};
use tool_cards::{finished_card, started_card, StartedCursorTool};

/// Bound on tool calls retained while in flight. Cursor runs observed in the
/// spike peaked at 33 sequential calls; parallel fan-out inside one turn is
/// the only way to exceed this, and eviction just loses card enrichment.
const MAX_ACTIVE_TOOLS: usize = 256;

/// Stateful mapper for one Cursor stream-json stdout session.
#[derive(Debug, Default)]
pub(crate) struct CursorStreamMapper {
    /// Assistant text accumulated since the last segment boundary.
    segment_text: String,
    /// Tool calls started and not yet completed, keyed by `call_id`.
    active_tools: HashMap<String, StartedCursorTool>,
    /// Workspace cwd from the init frame, used to compact card paths.
    cwd: Option<PathBuf>,
    step_started: bool,
}

impl CursorStreamMapper {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Map one stdout line. Blank lines produce nothing; malformed lines
    /// become a notice and never fail the run.
    pub(crate) fn push_line(&mut self, line: &str) -> Vec<StreamEffect> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(error) => return notice(format!("cursor stream: unparseable line: {error}")),
        };
        match decode_frame(value) {
            Ok(frame) => self.map_frame(frame),
            Err(error) => notice(format!("cursor stream: {error}")),
        }
    }

    fn map_frame(&mut self, frame: CursorFrame) -> Vec<StreamEffect> {
        match frame {
            CursorFrame::Init(init) => self.map_init(init),
            CursorFrame::User => Vec::new(),
            CursorFrame::ThinkingDelta(text) => self.with_step_started(reasoning_effects(&text)),
            CursorFrame::ThinkingCompleted => Vec::new(),
            CursorFrame::Assistant(frame) => self.map_assistant(frame),
            CursorFrame::ToolCall(frame) => self.map_tool_call(frame),
            CursorFrame::Result(result) => self.map_result(result),
            CursorFrame::Unknown { kind, subtype } => notice(match subtype {
                Some(subtype) => format!("cursor stream: unknown frame {kind}/{subtype}"),
                None => format!("cursor stream: unknown frame {kind}"),
            }),
        }
    }

    fn map_init(&mut self, init: InitFrame) -> Vec<StreamEffect> {
        self.cwd = init.cwd.map(PathBuf::from);
        vec![StreamEffect::Status(StatusPatch {
            state: Some(RunState::Running),
            last_activity: Some("cursor init".into()),
            // NEXT_MAJOR(result.json): rename claude_session_id/claude_model to runtime_session_id/runtime_model; readers branch on runtime.
            claude_session_id: init.session_id,
            claude_model: init
                .model
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty()),
            ..StatusPatch::default()
        })]
    }

    fn map_assistant(&mut self, frame: AssistantFrame) -> Vec<StreamEffect> {
        if frame.text.is_empty() {
            return Vec::new();
        }
        let is_snapshot = !self.segment_text.is_empty() && frame.text == self.segment_text;
        if is_snapshot {
            // Cumulative replay of what was already streamed. Drop it and
            // close the segment so the next delta starts fresh.
            self.segment_text.clear();
            return Vec::new();
        }
        if !frame.has_timestamp && !frame.has_model_call_id && !self.segment_text.is_empty() {
            // Snapshot-shaped frame whose text does not match what we saw:
            // schema drift. Render it (never drop real text) but say so.
            self.segment_text.clear();
            let mut effects =
                notice("cursor stream: final assistant frame did not match streamed deltas".into());
            effects.extend(text_effects(&frame.text));
            return self.with_step_started(effects);
        }
        self.segment_text.push_str(&frame.text);
        self.with_step_started(text_effects(&frame.text))
    }

    fn map_tool_call(&mut self, frame: ToolCallFrame) -> Vec<StreamEffect> {
        // A tool call always ends the current text segment.
        self.segment_text.clear();
        let cwd = self.cwd.as_deref();
        let ToolCallFrame {
            phase,
            call_id,
            tool_key,
            args,
            result,
        } = frame;
        match phase {
            ToolCallPhase::Started => {
                let tool = StartedCursorTool::new(&tool_key, args);
                let card = started_card(&tool, cwd);
                let verb = tool.verb();
                if self.active_tools.len() < MAX_ACTIVE_TOOLS {
                    self.active_tools.insert(call_id.clone(), tool);
                }
                self.with_step_started(vec![
                    StreamEffect::Attachment(AttachmentEvent::ToolStarted {
                        key: Some(call_id),
                        card,
                    }),
                    StreamEffect::Status(StatusPatch {
                        last_activity: Some(format!("tool: {verb}")),
                        ..StatusPatch::default()
                    }),
                ])
            }
            ToolCallPhase::Completed => {
                // `completed` repeats args, so an evicted or never-started
                // call still renders a full card.
                let tool = self
                    .active_tools
                    .remove(&call_id)
                    .unwrap_or_else(|| StartedCursorTool::new(&tool_key, args));
                let card = finished_card(&tool, result.as_ref(), cwd);
                vec![
                    StreamEffect::Attachment(AttachmentEvent::ToolFinished {
                        key: Some(call_id),
                        card,
                    }),
                    StreamEffect::Status(StatusPatch {
                        last_activity: Some(format!("tool result: {}", tool.verb())),
                        ..StatusPatch::default()
                    }),
                ]
            }
        }
    }

    fn map_result(&mut self, result: ResultFrame) -> Vec<StreamEffect> {
        self.segment_text.clear();
        let classification = classify_terminal_result(result.subtype.as_deref(), result.is_error);
        let usage = result.usage.as_ref().map(protocol::RawUsage::to_model);
        let input_tokens = usage.as_ref().and_then(ModelUsage::total_input_tokens);
        let output_tokens = usage.as_ref().and_then(|usage| usage.output_tokens);

        let mut effects = Vec::new();
        if let Some(usage) = usage.clone() {
            effects.push(StreamEffect::Attachment(AttachmentEvent::Usage(usage)));
        }

        // `result.result` is every text segment joined; the deltas already
        // rendered it, so it only lands on status, never as a text delta.
        let result_text = result.result.as_deref().map(bound_result_text);
        let error = match &classification {
            TerminalClassification::Success { .. } => None,
            TerminalClassification::Failure { subtype, .. } => Some(
                result_text
                    .clone()
                    .filter(|text: &String| !text.is_empty())
                    .unwrap_or_else(|| format!("cursor result subtype: {subtype}")),
            ),
            TerminalClassification::Invalid { reason } => Some(reason.clone()),
        };

        effects.push(StreamEffect::Status(StatusPatch {
            input_tokens,
            output_tokens,
            result: result_text.clone(),
            error: error.clone(),
            claude_session_id: result.session_id.clone(),
            last_activity: Some(match &classification {
                TerminalClassification::Success { .. } => "result received".into(),
                TerminalClassification::Failure { .. } => "result failed".into(),
                TerminalClassification::Invalid { .. } => "result invalid".into(),
            }),
            ..StatusPatch::default()
        }));
        effects.push(StreamEffect::Terminal(TerminalResult {
            classification,
            result_text,
            error,
            session_id: result.session_id,
            // Cursor reports no turn count; a run is one process = one turn.
            num_turns: Some(1),
            usage,
            context: None,
            total_cost_usd: None,
            permission_denials: Vec::new(),
            stop_reason: None,
        }));
        effects
    }

    /// Prefix `StepStarted` onto the first presentation effects of the run.
    fn with_step_started(&mut self, mut effects: Vec<StreamEffect>) -> Vec<StreamEffect> {
        if self.step_started || effects.is_empty() {
            return effects;
        }
        self.step_started = true;
        effects.insert(0, StreamEffect::Attachment(AttachmentEvent::StepStarted));
        effects
    }
}

impl StreamLineMapper for CursorStreamMapper {
    fn push_line(&mut self, line: &str) -> Vec<StreamEffect> {
        CursorStreamMapper::push_line(self, line)
    }
}

fn notice(message: String) -> Vec<StreamEffect> {
    vec![StreamEffect::Attachment(AttachmentEvent::Notice(message))]
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
