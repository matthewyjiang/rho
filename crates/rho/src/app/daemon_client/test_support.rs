//! Fixtures for the daemon clients' tests.

use rho_protocol::{
    JobState, ModelOptions, ModelRef, PermissionMode, PermissionModeOptions, Replay,
    SessionAttachResponse, SessionOptions, StreamId, TurnId,
};

/// An attach's response, on the stream named `"stream"`: a `replay`
/// replay of `replay_events` events, ending with the job in `state`, of
/// the turn named `turn` when it names one.
pub(crate) fn attach_response(
    replay: Replay,
    replay_events: u64,
    state: JobState,
    turn: Option<&str>,
) -> SessionAttachResponse {
    SessionAttachResponse {
        stream: StreamId::new("stream"),
        workspace: "/work".into(),
        replay,
        replay_events,
        state,
        turn: turn.map(TurnId::new),
        inputs: Vec::new(),
        title: None,
        options: SessionOptions {
            model: ModelOptions {
                auth: "api-key".into(),
                current: ModelRef {
                    provider: "test".into(),
                    model: "model".into(),
                },
                available: Vec::new(),
            },
            reasoning: None,
            fast_mode: None,
            permission_mode: PermissionModeOptions {
                current: PermissionMode::Bypass,
                available: Vec::new(),
            },
            commands: Vec::new(),
            goal: None,
        },
    }
}
