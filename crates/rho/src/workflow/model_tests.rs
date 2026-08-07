use pretty_assertions::assert_eq;

use super::{NodeState, NodeTerminalState, RunLifecycle, WorkflowOutcome};

/// The token serde would write for a unit-like value.
fn serialized(value: &impl serde::Serialize) -> String {
    let json = serde_json::to_value(value).expect("serializable");
    match json {
        serde_json::Value::String(token) => token,
        serde_json::Value::Object(fields) => fields
            .get("state")
            .and_then(serde_json::Value::as_str)
            .expect("internally tagged state")
            .to_owned(),
        other => panic!("unexpected serialized shape: {other}"),
    }
}

// Covers: the public workflow tokens must stay identical to the serialized
// form, so tool output, CLI output, and durable state cannot drift apart.
// Owner: workflow domain vocabulary.
#[test]
fn public_tokens_match_the_serialized_form() {
    for lifecycle in [
        RunLifecycle::Planned,
        RunLifecycle::Running,
        RunLifecycle::Cancelling,
        RunLifecycle::Completed,
        RunLifecycle::NeedsRecovery,
    ] {
        assert_eq!(lifecycle.as_str(), serialized(&lifecycle), "{lifecycle:?}");
    }

    for outcome in [
        WorkflowOutcome::Success,
        WorkflowOutcome::Failure,
        WorkflowOutcome::Denial,
        WorkflowOutcome::Cancellation,
        WorkflowOutcome::Blocked,
    ] {
        assert_eq!(outcome.as_str(), serialized(&outcome), "{outcome:?}");
    }

    for terminal in [
        NodeTerminalState::Success,
        NodeTerminalState::Failure,
        NodeTerminalState::Denial,
        NodeTerminalState::Cancellation,
        NodeTerminalState::Skipped,
        NodeTerminalState::Blocked,
    ] {
        assert_eq!(terminal.as_str(), serialized(&terminal), "{terminal:?}");
        assert_eq!(
            NodeState::Terminal { outcome: terminal }.as_str(),
            terminal.as_str(),
            "{terminal:?} flattened through NodeState"
        );
    }

    for state in [
        NodeState::Pending,
        NodeState::Ready,
        NodeState::Running {
            attempt: 1.try_into().expect("attempt"),
        },
    ] {
        assert_eq!(state.as_str(), serialized(&state), "{state:?}");
    }
}
