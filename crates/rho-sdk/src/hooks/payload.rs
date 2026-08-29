use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{
    workspace::{
        CapabilityOperation, CapabilityRequest, CapabilitySource, NetworkTarget, PathScope,
        ProcessEnvironment, ProcessExecution, ProcessInvocation,
    },
    StopReason,
};

use crate::workspace::PolicyDecision;

use super::bounds::{bounded_path, bounded_string, HookPayloadBounds, HookTruncation};

/// Which configured tool the event is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HookTool {
    /// Canonical tool name. Matchers compare against this, never a display name.
    pub name: String,
    /// Identity of the individual call, present for tool events inside a run.
    pub call_id: Option<String>,
}

/// Name reported when a capability comes from prompt construction rather than a
/// tool call. It is not a valid tool name, so no `tools` matcher selects it.
pub const PROMPT_CONSTRUCTION_TOOL: &str = "<prompt>";

/// Filesystem scope a path was resolved in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookPathScope {
    PrimaryWorkspace,
    GrantedRoot,
    UnrestrictedFilesystem,
}

/// Ambient environment class handed to a child process.
///
/// Variable *values* are never included. A hook that needs a value must read it
/// from its own allowlisted environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookProcessEnvironment {
    Empty,
    InheritAll,
    InheritListed,
    InheritExcept,
}

/// Safe structured summary of the authority a tool asked for.
///
/// Built from the already-structured capability request, not by scraping
/// argument prose. Paths and command text are included because deny hooks exist
/// to inspect them; credentials, headers, and environment values are not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookCapability {
    ReadPath {
        path: PathBuf,
        scope: HookPathScope,
    },
    WritePath {
        path: PathBuf,
        scope: HookPathScope,
    },
    ExecuteProcess {
        working_directory: PathBuf,
        executable: PathBuf,
        arguments: Vec<String>,
        /// Script text, present only when execution really goes through a shell.
        shell_command: Option<String>,
        environment: HookProcessEnvironment,
    },
    NetworkAccess {
        /// Destination with userinfo and query string removed.
        url: Option<String>,
        host: Option<String>,
        /// Whether the original URL carried a query string.
        query_present: bool,
    },
    LoadSkill {
        name: String,
        path: Option<PathBuf>,
    },
    DiscoverInstructions {
        path: PathBuf,
        scope: HookPathScope,
    },
}

/// Host policy result a blocking hook is allowed to see.
///
/// A denied capability never reaches a hook, so this never reports `deny`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookPolicyOutcome {
    Allow,
    RequireApproval,
}

impl HookPolicyOutcome {
    /// Returns the outcome a hook may observe, or `None` when policy already denied.
    pub(crate) fn from_policy(decision: &PolicyDecision) -> Option<Self> {
        match decision {
            PolicyDecision::Allow => Some(Self::Allow),
            PolicyDecision::RequireApproval { .. } => Some(Self::RequireApproval),
            PolicyDecision::Deny { .. } => None,
        }
    }
}

/// How a tool call resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookToolStatus {
    Succeeded,
    Failed,
    Unavailable,
}

/// Sanitized failure detail for a post-action event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HookFailure {
    pub kind: String,
    pub message: String,
}

pub(crate) struct BoundedFailure<'a> {
    pub(crate) kind: &'a str,
    pub(crate) message: &'a str,
    pub(crate) field: &'a str,
}

/// Why a successful run stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookStopReason {
    EndTurn,
    MaxSteps,
}

impl From<StopReason> for HookStopReason {
    fn from(reason: StopReason) -> Self {
        match reason {
            StopReason::EndTurn => Self::EndTurn,
            StopReason::MaxSteps => Self::MaxSteps,
        }
    }
}

/// Event-specific body of a [`HookEnvelope`](super::HookEnvelope).
///
/// Variants exist only for delivered events. The event kind on the envelope
/// selects which shape a handler should read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum HookPayload {
    SessionStarted(SessionStartedPayload),
    BeforeToolUse(BeforeToolUsePayload),
    AfterToolUse(AfterToolUsePayload),
    RunCompleted(RunCompletedPayload),
    RunFailed(RunFailedPayload),
    SessionCompleted(SessionCompletedPayload),
    SessionFailed(SessionFailedPayload),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionStartedPayload {
    /// Model identity the session was created with, for example `anthropic/opus`.
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BeforeToolUsePayload {
    pub tool: HookTool,
    pub capability: HookCapability,
    pub policy: HookPolicyOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AfterToolUsePayload {
    pub tool: HookTool,
    /// First capability this call requested, the same summary `before_tool_use` saw.
    ///
    /// `None` when the call requested no capability or failed before producing one.
    /// Multi-capability calls report only the first; later requests still appear as
    /// their own `before_tool_use` events.
    pub capability: Option<HookCapability>,
    pub status: HookToolStatus,
    pub failure: Option<HookFailure>,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunCompletedPayload {
    pub stop_reason: HookStopReason,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunFailedPayload {
    pub failure: HookFailure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionCompletedPayload {
    pub runs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionFailedPayload {
    pub failure: HookFailure,
}

impl HookPayload {
    pub(crate) const fn event(&self) -> super::HookEventKind {
        match self {
            Self::SessionStarted(_) => super::HookEventKind::SessionStarted,
            Self::BeforeToolUse(_) => super::HookEventKind::BeforeToolUse,
            Self::AfterToolUse(_) => super::HookEventKind::AfterToolUse,
            Self::RunCompleted(_) => super::HookEventKind::RunCompleted,
            Self::RunFailed(_) => super::HookEventKind::RunFailed,
            Self::SessionCompleted(_) => super::HookEventKind::SessionCompleted,
            Self::SessionFailed(_) => super::HookEventKind::SessionFailed,
        }
    }

    /// Tool name a `tools` matcher compares against, when the event has one.
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            Self::BeforeToolUse(payload) => Some(payload.tool.name.as_str()),
            Self::AfterToolUse(payload) => Some(payload.tool.name.as_str()),
            Self::SessionStarted(_)
            | Self::RunCompleted(_)
            | Self::RunFailed(_)
            | Self::SessionCompleted(_)
            | Self::SessionFailed(_) => None,
        }
    }

    /// Whether the operation this event reports succeeded.
    ///
    /// `None` for events that report no outcome.
    pub fn succeeded(&self) -> Option<bool> {
        match self {
            Self::AfterToolUse(payload) => {
                Some(matches!(payload.status, HookToolStatus::Succeeded))
            }
            Self::RunCompleted(_) | Self::SessionCompleted(_) => Some(true),
            Self::RunFailed(_) | Self::SessionFailed(_) => Some(false),
            Self::SessionStarted(_) | Self::BeforeToolUse(_) => None,
        }
    }
}

impl HookTool {
    pub(crate) fn new(
        name: impl Into<String>,
        call_id: Option<String>,
        bounds: HookPayloadBounds,
        truncation: &mut HookTruncation,
    ) -> Self {
        Self {
            name: bounded_string(name, "payload.tool.name", bounds, truncation),
            call_id: call_id
                .map(|call_id| bounded_string(call_id, "payload.tool.call_id", bounds, truncation)),
        }
    }

    pub(crate) fn source_name(source: &CapabilitySource) -> &str {
        match source {
            CapabilitySource::BuiltInTool { name }
            | CapabilitySource::HostProvidedTool { name } => name.as_str(),
            CapabilitySource::PromptConstruction => PROMPT_CONSTRUCTION_TOOL,
        }
    }

    pub(crate) fn from_source(
        source: &CapabilitySource,
        call_id: Option<String>,
        bounds: HookPayloadBounds,
        truncation: &mut HookTruncation,
    ) -> Self {
        Self::new(Self::source_name(source), call_id, bounds, truncation)
    }
}

impl From<&PathScope> for HookPathScope {
    fn from(scope: &PathScope) -> Self {
        match scope {
            PathScope::PrimaryWorkspace => Self::PrimaryWorkspace,
            PathScope::GrantedRoot { .. } => Self::GrantedRoot,
            PathScope::UnrestrictedFilesystem => Self::UnrestrictedFilesystem,
        }
    }
}

impl From<&ProcessEnvironment> for HookProcessEnvironment {
    fn from(environment: &ProcessEnvironment) -> Self {
        match environment {
            ProcessEnvironment::Empty => Self::Empty,
            ProcessEnvironment::InheritAll => Self::InheritAll,
            ProcessEnvironment::InheritListed { .. } => Self::InheritListed,
            ProcessEnvironment::InheritExcept { .. } => Self::InheritExcept,
        }
    }
}

pub(crate) fn summarize_capability(
    request: &CapabilityRequest,
    bounds: HookPayloadBounds,
    truncation: &mut HookTruncation,
) -> HookCapability {
    match request.operation() {
        CapabilityOperation::ReadPath { path, scope } => HookCapability::ReadPath {
            path: bounded_path(path, "payload.capability.path", bounds, truncation),
            scope: scope.into(),
        },
        CapabilityOperation::WritePath { path, scope } => HookCapability::WritePath {
            path: bounded_path(path, "payload.capability.path", bounds, truncation),
            scope: scope.into(),
        },
        CapabilityOperation::DiscoverInstructions { path, scope } => {
            HookCapability::DiscoverInstructions {
                path: bounded_path(path, "payload.capability.path", bounds, truncation),
                scope: scope.into(),
            }
        }
        CapabilityOperation::ExecuteProcess(execution) => {
            summarize_process(execution, bounds, truncation)
        }
        CapabilityOperation::NetworkAccess(target) => summarize_network(target, bounds, truncation),
        CapabilityOperation::LoadSkill { name, path } => HookCapability::LoadSkill {
            name: bounded_string(name.clone(), "payload.capability.name", bounds, truncation),
            path: path
                .as_deref()
                .map(|path| bounded_path(path, "payload.capability.path", bounds, truncation)),
        },
    }
}

fn summarize_process(
    execution: &ProcessExecution,
    bounds: HookPayloadBounds,
    truncation: &mut HookTruncation,
) -> HookCapability {
    let invocation = execution.invocation();
    let mut arguments = Vec::new();
    let mut argument_bytes = 0usize;
    for argument in invocation.arguments() {
        let argument = bounded_string(
            argument.clone(),
            &format!("payload.capability.arguments[{}]", arguments.len()),
            bounds,
            truncation,
        );
        if argument_bytes.saturating_add(argument.len()) > bounds.max_envelope_bytes() {
            truncation.record("payload.capability.arguments");
            break;
        }
        argument_bytes += argument.len();
        arguments.push(argument);
    }
    let shell_command = match invocation {
        ProcessInvocation::Shell { command, .. } => Some(bounded_string(
            command.clone(),
            "payload.capability.shell_command",
            bounds,
            truncation,
        )),
        ProcessInvocation::Executable { .. } => None,
    };
    HookCapability::ExecuteProcess {
        working_directory: bounded_path(
            execution.working_directory(),
            "payload.capability.working_directory",
            bounds,
            truncation,
        ),
        executable: bounded_path(
            invocation.executable_path(),
            "payload.capability.executable",
            bounds,
            truncation,
        ),
        arguments,
        shell_command,
        environment: execution.environment().into(),
    }
}

fn summarize_network(
    target: &NetworkTarget,
    bounds: HookPayloadBounds,
    truncation: &mut HookTruncation,
) -> HookCapability {
    let Some(raw) = target.url() else {
        return HookCapability::NetworkAccess {
            url: None,
            host: None,
            query_present: false,
        };
    };
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return HookCapability::NetworkAccess {
            url: None,
            host: None,
            query_present: false,
        };
    };
    let query_present = parsed.query().is_some();
    // Credentials and query strings routinely carry tokens; the host and path
    // are what a network hook needs.
    parsed.set_query(None);
    parsed.set_fragment(None);
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    HookCapability::NetworkAccess {
        host: parsed
            .host_str()
            .map(|host| bounded_string(host, "payload.capability.host", bounds, truncation)),
        url: Some(bounded_string(
            parsed.to_string(),
            "payload.capability.url",
            bounds,
            truncation,
        )),
        query_present,
    }
}

/// Stable snake_case classification for a run or session failure.
///
/// Hook matchers and dashboards key on this instead of parsing message text.
pub(crate) fn error_label(error: &crate::Error) -> &'static str {
    match error {
        crate::Error::InvalidConfiguration { .. } => "invalid_configuration",
        crate::Error::Authentication { .. } => "authentication",
        crate::Error::Provider(_) => "provider",
        crate::Error::Tool(_) => "tool",
        crate::Error::Persistence { .. } => "persistence",
        crate::Error::PolicyDenied { .. } => "policy_denied",
        crate::Error::RuntimeShutdown => "runtime_shutdown",
        crate::Error::SessionBusy => "session_busy",
        crate::Error::Cancelled => "cancelled",
        crate::Error::Interrupted { .. } => "interrupted",
        crate::Error::InvalidHostResponse { .. } => "invalid_host_response",
    }
}

/// Shortens a failure message so a long tool error cannot inflate an envelope.
pub(crate) fn bounded_failure(
    failure: BoundedFailure<'_>,
    bounds: HookPayloadBounds,
    truncation: &mut HookTruncation,
) -> HookFailure {
    HookFailure {
        kind: bounded_string(
            failure.kind,
            &format!("{}.kind", failure.field),
            bounds,
            truncation,
        ),
        message: bounded_string(
            failure.message,
            &format!("{}.message", failure.field),
            bounds,
            truncation,
        ),
    }
}

/// Workspace identity carried by every envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HookWorkspace {
    pub root: Option<PathBuf>,
}

impl HookWorkspace {
    pub(crate) fn from_root(
        root: Option<&Path>,
        bounds: HookPayloadBounds,
        truncation: &mut HookTruncation,
    ) -> Self {
        Self {
            root: root.map(|root| bounded_path(root, "workspace.root", bounds, truncation)),
        }
    }
}

#[cfg(test)]
#[path = "payload_tests.rs"]
mod tests;
