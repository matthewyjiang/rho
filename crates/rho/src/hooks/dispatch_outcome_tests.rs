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

/// Names every unit variant of `$ty` once.
///
/// The generated match is exhaustive, so a new domain variant fails to compile
/// until it is listed here. The same list becomes the array that feeds the
/// token comparison against the hooks allow-list - one update site, not two.
macro_rules! exhaustive_variants {
    ($ty:ty; $($variant:ident),+ $(,)?) => {{
        const _: fn($ty) = |value| match value {
            $(<$ty>::$variant => {},)+
        };
        [$(<$ty>::$variant),+]
    }};
}

// Covers: a new or renamed NodeTerminalState must stay on the hooks node
// outcome allow-list, or NodeFinished hooks are dropped silently at runtime.
// Owner: hooks workflow outcome boundary.
#[test]
fn frozen_node_outcomes_match_every_node_terminal_state() {
    let from_domain = exhaustive_variants!(
        NodeTerminalState;
        Success,
        Failure,
        Denial,
        Cancellation,
        Skipped,
        Blocked,
    )
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
    // Every variant is listed once. The loop match is also exhaustive, so a new
    // outcome must be classified as a Failed-hook token or as a fixed-literal
    // path. Failure-arm order matches WORKFLOW_FAILURE_OUTCOMES.
    let mut from_domain = Vec::new();
    for outcome in exhaustive_variants!(
        WorkflowOutcome;
        Denial,
        Failure,
        Blocked,
        Success,
        Cancellation,
    ) {
        match outcome {
            WorkflowOutcome::Success => {
                assert_eq!(serialized_token(&outcome), "success");
            }
            WorkflowOutcome::Cancellation => {
                assert_eq!(serialized_token(&outcome), "cancellation");
            }
            WorkflowOutcome::Denial | WorkflowOutcome::Failure | WorkflowOutcome::Blocked => {
                from_domain.push(serialized_token(&outcome));
            }
        }
    }

    assert_eq!(
        from_domain
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice(),
        WORKFLOW_FAILURE_OUTCOMES,
        "update WORKFLOW_FAILURE_OUTCOMES when WorkflowOutcome failure variants change"
    );
}
