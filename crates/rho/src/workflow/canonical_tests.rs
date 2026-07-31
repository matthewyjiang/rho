use super::*;
use crate::workflow::{
    test_support::{agent_node, workflow},
    WorkspaceAccess,
};

// Covers: JSON formatting or map order could change plan identity and break exact resume.
// Owner: workflow canonical encoding.
#[test]
fn representative_graph_has_stable_binary_digest() {
    let workflow = workflow(vec![agent_node("inspect", &[], WorkspaceAccess::ReadOnly)]);
    assert_eq!(
        graph_digest(&workflow).unwrap().0,
        "sha256:6009f540a208ad2f40b7dd18fc869d56a6d012149444861bc81aebd5d5a9ce39"
    );
    assert!(canonical_bytes(&workflow).unwrap().starts_with(DOMAIN));
}
