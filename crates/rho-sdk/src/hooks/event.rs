use serde::Serialize;

/// Every lifecycle moment Rho can name for a hook.
///
/// The enum is the single place that defines the hook event vocabulary. It
/// deliberately names more moments than the runtime delivers so configuration
/// diagnostics can report "known but not delivered" instead of "unknown event".
/// [`HookEventKind::is_delivered`] is the gate; [`HookEventKind::is_blocking`]
/// separates the one pre-action event whose decision can stop work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookEventKind {
    /// A session was created on a runtime.
    SessionStarted,
    /// A capability-bearing tool call is about to be authorized. Blocking.
    BeforeToolUse,
    /// A tool call resolved, successfully or not.
    AfterToolUse,
    /// One run finished with a typed outcome.
    RunCompleted,
    /// One run ended in an error.
    RunFailed,
    /// The host closed a session normally.
    SessionCompleted,
    /// The host closed a session after a failure.
    SessionFailed,
    /// Named but not delivered: user input was accepted for a run.
    UserPromptAccepted,
    /// Named but not delivered: a provider request is about to be sent.
    BeforeModelRequest,
    /// Named but not delivered: a provider response was normalized.
    ModelResponseCompleted,
    /// Named but not delivered: one model step finished.
    TurnCompleted,
}

impl HookEventKind {
    /// Every named event, in wire-documentation order.
    pub const ALL: &'static [Self] = &[
        Self::SessionStarted,
        Self::BeforeToolUse,
        Self::AfterToolUse,
        Self::RunCompleted,
        Self::RunFailed,
        Self::SessionCompleted,
        Self::SessionFailed,
        Self::UserPromptAccepted,
        Self::BeforeModelRequest,
        Self::ModelResponseCompleted,
        Self::TurnCompleted,
    ];

    /// Stable serialized name. This is the value hosts accept in configuration.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::SessionStarted => "session_started",
            Self::BeforeToolUse => "before_tool_use",
            Self::AfterToolUse => "after_tool_use",
            Self::RunCompleted => "run_completed",
            Self::RunFailed => "run_failed",
            Self::SessionCompleted => "session_completed",
            Self::SessionFailed => "session_failed",
            Self::UserPromptAccepted => "user_prompt_accepted",
            Self::BeforeModelRequest => "before_model_request",
            Self::ModelResponseCompleted => "model_response_completed",
            Self::TurnCompleted => "turn_completed",
        }
    }

    /// Resolves a configured event name.
    pub fn from_wire_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|event| event.wire_name() == name)
    }

    /// Whether the runtime dispatches this event at schema version 1.
    ///
    /// Undelivered variants exist so hosts can reject them by name. They carry
    /// no payload type and no mutate or inject API.
    pub const fn is_delivered(self) -> bool {
        match self {
            Self::SessionStarted
            | Self::BeforeToolUse
            | Self::AfterToolUse
            | Self::RunCompleted
            | Self::RunFailed
            | Self::SessionCompleted
            | Self::SessionFailed => true,
            Self::UserPromptAccepted
            | Self::BeforeModelRequest
            | Self::ModelResponseCompleted
            | Self::TurnCompleted => false,
        }
    }

    /// Whether a hook decision for this event can stop the operation.
    ///
    /// Only [`HookEventKind::BeforeToolUse`] is blocking. Every other event is
    /// observational: its handler result cannot change what the agent does.
    pub const fn is_blocking(self) -> bool {
        matches!(self, Self::BeforeToolUse)
    }

    /// Whether this event reports the result of an operation.
    ///
    /// Post events carry a status a matcher may filter on.
    pub const fn is_post_action(self) -> bool {
        matches!(
            self,
            Self::AfterToolUse | Self::RunCompleted | Self::RunFailed | Self::SessionFailed
        )
    }
}

impl std::fmt::Display for HookEventKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.wire_name())
    }
}

#[cfg(test)]
#[path = "event_tests.rs"]
mod tests;
