use super::{
    find_thermos_workflow, parse_thermos_args, ThermosArgsError, ThermosRequest, ThermosScope,
};
use crate::tui::workflow_discover::DiscoveredWorkflow;
use crate::workflow::{InputName, WorkflowValue};
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn source(label: &str, relative: &str) -> DiscoveredWorkflow {
    DiscoveredWorkflow {
        relative_path: relative.into(),
        absolute_path: PathBuf::from(relative),
        label: label.into(),
    }
}

// Covers: /thermos args must map to workflow inputs without inventing flags.
// Owner: thermos command policy
#[test]
fn parse_thermos_args_maps_scope_and_optional_path() {
    let cases = [
        (
            "",
            Ok(ThermosRequest {
                scope: ThermosScope::All,
                focus_path: None,
            }),
        ),
        (
            "committed",
            Ok(ThermosRequest {
                scope: ThermosScope::Committed,
                focus_path: None,
            }),
        ),
        (
            "UNCOMMITTED crates/rho/src/tui",
            Ok(ThermosRequest {
                scope: ThermosScope::Uncommitted,
                focus_path: Some("crates/rho/src/tui".into()),
            }),
        ),
        (
            "crates/rho/src/tui",
            Ok(ThermosRequest {
                scope: ThermosScope::All,
                focus_path: Some("crates/rho/src/tui".into()),
            }),
        ),
        ("--base main", Err(ThermosArgsError::InvalidFlag)),
        ("committed --help", Err(ThermosArgsError::InvalidFlag)),
    ];

    for (args, expected) in cases {
        assert_eq!(parse_thermos_args(args), expected, "{args:?}");
    }
}

// Covers: explicit non-default inputs must be the only keys sent to planning.
// Owner: thermos command policy
#[test]
fn thermos_request_omits_default_scope_from_inputs() {
    let default_inputs = ThermosRequest {
        scope: ThermosScope::All,
        focus_path: None,
    }
    .into_inputs();
    assert_eq!(default_inputs, BTreeMap::new());

    let committed = ThermosRequest {
        scope: ThermosScope::Committed,
        focus_path: Some("crates/rho".into()),
    }
    .into_inputs();
    assert_eq!(
        committed,
        BTreeMap::from([
            (
                InputName::new("scope").unwrap(),
                WorkflowValue::String("committed".into())
            ),
            (
                InputName::new("focus_path").unwrap(),
                WorkflowValue::String("crates/rho".into())
            ),
        ])
    );
}

// Covers: /thermos must bind the canonical review workflow, not the first
// discovered source, and must prefer thermo-nuclear-review when both exist.
// Owner: thermos command policy
#[test]
fn find_thermos_workflow_prefers_canonical_label() {
    assert_eq!(
        find_thermos_workflow(&[]).map(|source| source.label.as_str()),
        None
    );
    assert_eq!(
        find_thermos_workflow(&[source("review", ".rho/workflows/review/workflow.star")])
            .map(|source| source.label.as_str()),
        None
    );
    assert_eq!(
        find_thermos_workflow(&[source("thermos", ".rho/workflows/thermos.star")])
            .map(|source| source.relative_path.as_str()),
        Some(".rho/workflows/thermos.star")
    );

    let both = [
        source("thermos", ".rho/workflows/thermos.star"),
        source(
            "thermo-nuclear-review",
            ".rho/workflows/thermo-nuclear-review/workflow.star",
        ),
    ];
    assert_eq!(
        find_thermos_workflow(&both).map(|source| source.relative_path.as_str()),
        Some(".rho/workflows/thermo-nuclear-review/workflow.star")
    );
}
