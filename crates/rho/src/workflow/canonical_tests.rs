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
        "sha256:0298a1dfb55e7c651c522fb6a3dfc3ac3227d025fd8298a3f866b2f1a5165d7e"
    );
    assert!(canonical_bytes(&workflow).unwrap().starts_with(DOMAIN));
}
