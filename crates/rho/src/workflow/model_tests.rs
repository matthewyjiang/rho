use pretty_assertions::assert_eq;

use super::{
    NodeState, NodeTerminalState, RunLifecycle, ScopeInstanceId, TaskInstanceId, WorkflowOutcome,
};

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
        let state = NodeState::Terminal { outcome: terminal };
        assert_eq!(
            state.as_str(),
            serialized(&terminal),
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

// Covers: same-named tasks in distinct scopes cannot collide in JSON maps or paths.
#[test]
fn qualified_identity_roundtrips_map_keys_and_rejects_noncanonical_paths() {
    let definition = crate::workflow::NodeId::new("task-with-dashes").unwrap();
    let tasks = std::collections::BTreeMap::from([
        (TaskInstanceId::root(definition.clone()), 1),
        (
            TaskInstanceId::new(ScopeInstanceId::new(u64::MAX), definition),
            2,
        ),
    ]);
    let encoded = serde_json::to_string(&tasks).unwrap();
    assert_eq!(
        serde_json::from_str::<std::collections::BTreeMap<TaskInstanceId, i32>>(&encoded).unwrap(),
        tasks
    );
    assert_eq!(tasks.keys().next().unwrap().to_string(), "task-with-dashes");
    assert_eq!(
        tasks.keys().nth(1).unwrap().to_string(),
        "s18446744073709551615.task-with-dashes"
    );
    for invalid in [
        "s0.task",
        "s00.task",
        "s+1.task",
        "s01.task",
        "s18446744073709551616.task",
        "s1.../task",
        "s1.task/name",
        "s1.task\\name",
        "s1.",
        "../task",
        "",
    ] {
        assert!(invalid.parse::<TaskInstanceId>().is_err(), "{invalid}");
    }
}
