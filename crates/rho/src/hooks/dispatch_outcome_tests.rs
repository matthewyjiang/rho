//! Drift guards for the frozen workflow outcome allow-lists.

use pretty_assertions::assert_eq;
use serde::Serialize;

use super::{NODE_OUTCOMES, WORKFLOW_FAILURE_OUTCOMES};
use crate::workflow::{NodeTerminalState, WorkflowOutcome};

/// The string token serde writes for a unit-like outcome enum.
fn serialized_token(value: &impl Serialize) -> String {
    let json = serde_json::to_value(value).expect("serializable");
    json.as_str()
        .unwrap_or_else(|| panic!("expected a string outcome token, got {json}"))
        .to_owned()
}

// Covers: a new or renamed NodeTerminalState must stay on the hooks node
// outcome allow-list, or NodeFinished hooks are dropped silently at runtime.
// Owner: hooks workflow outcome boundary.
#[test]
fn frozen_node_outcomes_match_every_node_terminal_state() {
    let from_domain = [
        NodeTerminalState::Success,
        NodeTerminalState::Failure,
        NodeTerminalState::Denial,
        NodeTerminalState::Cancellation,
        NodeTerminalState::Skipped,
        NodeTerminalState::Blocked,
    ]
    .map(|outcome| serialized_token(&outcome));

    assert_eq!(
        from_domain
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice(),
        NODE_OUTCOMES,
        "update NODE_OUTCOMES when NodeTerminalState gains, loses, or renames a variant"
    );
}

// Covers: Failed workflow hooks must accept every non-success,
// non-cancellation WorkflowOutcome, or those completions drop silently.
// Success and Cancellation use fixed literals on their own observe paths.
// Owner: hooks workflow outcome boundary.
#[test]
fn frozen_workflow_failure_outcomes_match_failed_workflow_outcomes() {
    let from_domain = [
        WorkflowOutcome::Denial,
        WorkflowOutcome::Failure,
        WorkflowOutcome::Blocked,
    ]
    .map(|outcome| serialized_token(&outcome));

    assert_eq!(
        from_domain
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice(),
        WORKFLOW_FAILURE_OUTCOMES,
        "update WORKFLOW_FAILURE_OUTCOMES when WorkflowOutcome failure variants change"
    );

    // Keep the fixed completed/cancelled literals honest against the domain.
    assert_eq!(serialized_token(&WorkflowOutcome::Success), "success");
    assert_eq!(
        serialized_token(&WorkflowOutcome::Cancellation),
        "cancellation"
    );
}
