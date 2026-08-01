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
        "sha256:d293aa54019294b40bde05d7169a91d55d7e383c774bda22821222b879570fcd"
    );
    assert!(canonical_bytes(&workflow).unwrap().starts_with(DOMAIN));
}
