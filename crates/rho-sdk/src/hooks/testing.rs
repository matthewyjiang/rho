//! Envelope constructors for host test suites.
//!
//! Hosts implementing [`PreToolUseGate`](super::PreToolUseGate) or
//! [`HookObserver`](super::HookObserver) need real envelopes to test against.
//! Building one by hand would duplicate the wire contract, so the SDK supplies
//! these instead of widening the production constructors.

use super::{
    bounds::HookPayloadBounds,
    envelope::{HookEnvelope, HookEnvelopeBuilder, HookIdentity},
    gate::PreToolUseRequest,
    payload::{
        summarize_capability, AfterToolUsePayload, BeforeToolUsePayload, HookPayload,
        HookPolicyOutcome, HookStopReason, HookTool, HookToolStatus, RunCompletedPayload,
    },
};
use crate::{
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
    let bounds = HookPayloadBounds::default();
    let mut builder = HookEnvelopeBuilder::new(identity(), None, bounds);
    let tool = HookTool::new(tool, Some("test-call".into()), bounds, builder.truncation());
    builder.finish(HookPayload::AfterToolUse(AfterToolUsePayload {
        tool,
        capability: None,
        status: HookToolStatus::Succeeded,
        failure: None,
        duration_ms: Some(1),
    }))
}

/// An `after_tool_use` envelope for `tool` that ran `command` through a shell.
pub fn after_tool_use_envelope_for_command(tool: &str, command: &str) -> HookEnvelope {
    let request = process_request(tool, command);
    let bounds = HookPayloadBounds::default();
    let mut builder = HookEnvelopeBuilder::new(identity(), None, bounds);
    let capability = summarize_capability(&request, bounds, builder.truncation());
    let tool = HookTool::new(tool, Some("test-call".into()), bounds, builder.truncation());
    builder.finish(HookPayload::AfterToolUse(AfterToolUsePayload {
        tool,
        capability: Some(capability),
        status: HookToolStatus::Succeeded,
        failure: None,
        duration_ms: Some(1),
    }))
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
