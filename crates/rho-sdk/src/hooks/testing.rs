//! Envelope constructors for host test suites.
//!
//! Hosts implementing [`PreToolUseGate`](super::PreToolUseGate) or
//! [`HookObserver`] need real envelopes to test against.
//! Building one by hand would duplicate the wire contract, so the SDK supplies
//! these instead of widening the production constructors. [`RecordingObserver`]
//! captures what a runtime delivers.

use std::sync::{Mutex, PoisonError};

use super::{
    bounds::HookPayloadBounds,
    dispatch::HookObserver,
    envelope::{HookEnvelope, HookEnvelopeBuilder, HookIdentity},
    event::HookEventKind,
    gate::PreToolUseRequest,
    payload::{
        summarize_capability, BeforeToolUsePayload, HookPayload, HookPolicyOutcome, HookStopReason,
        HookTool, RunCompletedPayload, ToolOutcomeRef,
    },
};
use crate::{
    tool::{ProcessResult, ToolOutput},
    workspace::{
        CapabilityRequest, CapabilitySource, ProcessEnvironment, ProcessExecution,
        ProcessInvocation, ProcessOutputLimits,
    },
    RunId, SessionId,
};

fn identity() -> HookIdentity {
    HookIdentity {
        session_id: Some(SessionId::from_string("test-session").expect("nonempty")),
        parent_session_id: None,
        run_id: Some(RunId::from_string("test-run").expect("nonempty")),
    }
}

fn process_request(tool: &str, command: &str) -> CapabilityRequest {
    CapabilityRequest::process(
        ProcessExecution::new(
            "/work",
            ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], command),
            ProcessEnvironment::Empty,
            ProcessOutputLimits::new(1024, None),
        ),
        CapabilitySource::built_in_tool(tool),
    )
}

/// A `before_tool_use` envelope for `tool` running `command` through a shell.
pub fn before_tool_use_envelope(tool: &str, command: &str) -> HookEnvelope {
    let request = process_request(tool, command);
    let bounds = HookPayloadBounds::default();
    let mut builder = HookEnvelopeBuilder::new(identity(), None, bounds);
    let capability = summarize_capability(&request, bounds, builder.truncation());
    let tool = HookTool::new(tool, Some("test-call".into()), bounds, builder.truncation());
    builder.finish(HookPayload::BeforeToolUse(BeforeToolUsePayload {
        tool,
        capability,
        policy: HookPolicyOutcome::Allow,
    }))
}

/// A `before_tool_use` request a gate under test can be handed directly.
pub fn before_tool_use_request(tool: &str, policy: HookPolicyOutcome) -> PreToolUseRequest {
    PreToolUseRequest::new(before_tool_use_envelope(tool, "git push --force"), policy)
}

/// An `after_tool_use` envelope reporting a successful call of `tool`.
pub fn after_tool_use_envelope(tool: &str) -> HookEnvelope {
    after_tool_use_output_envelope(tool, &ToolOutput::text(""))
}

/// An `after_tool_use` envelope for a call of `tool` that ran `process` to
/// completion.
///
/// The call succeeded when the exit code is `0`. Otherwise it failed and
/// reports the process's stderr as the failure message.
pub fn after_tool_use_process_envelope(tool: &str, process: ProcessResult) -> HookEnvelope {
    let succeeded = process.exit_code() == Some(0);
    let output = ToolOutput::text(process.stderr()).with_process_result(process);
    let output = if succeeded { output } else { output.failed() };
    after_tool_use_output_envelope(tool, &output)
}

fn after_tool_use_output_envelope(tool: &str, output: &ToolOutput) -> HookEnvelope {
    let bounds = HookPayloadBounds::default();
    let mut builder = HookEnvelopeBuilder::new(identity(), None, bounds);
    let tool = HookTool::new(tool, Some("test-call".into()), bounds, builder.truncation());
    builder.finish_tool_call(
        tool,
        ToolOutcomeRef::Completed(output),
        /* duration_ms */ Some(1),
        /* capability */ None,
    )
}

/// A `run_completed` envelope for an end-turn run.
pub fn run_completed_envelope() -> HookEnvelope {
    HookEnvelopeBuilder::new(identity(), None, HookPayloadBounds::default()).finish(
        HookPayload::RunCompleted(RunCompletedPayload {
            stop_reason: HookStopReason::EndTurn,
            revision: 1,
        }),
    )
}

/// Observer that keeps every envelope it is handed, for assertions.
///
/// Install it with `Rho::builder().hook_observer_shared(..)`, run the session,
/// then read [`Self::envelopes`] or [`Self::events`].
#[derive(Debug, Default)]
pub struct RecordingObserver {
    seen: Mutex<Vec<HookEnvelope>>,
}

impl RecordingObserver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every envelope observed so far, in delivery order.
    pub fn envelopes(&self) -> Vec<HookEnvelope> {
        self.lock().clone()
    }

    /// The event kind of every envelope observed so far, in delivery order.
    pub fn events(&self) -> Vec<HookEventKind> {
        self.lock().iter().map(HookEnvelope::event).collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<HookEnvelope>> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl HookObserver for RecordingObserver {
    fn observe(&self, envelope: HookEnvelope) {
        self.lock().push(envelope);
    }
}
