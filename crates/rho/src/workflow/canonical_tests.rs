use super::*;
use crate::workflow::{
    test_support::{agent_node, workflow},
    WorkspaceAccess,
};

// Covers: JSON formatting or map order could change plan identity and break exact resume.
// Owner: workflow canonical encoding.
#[test]
fn representative_program_has_stable_binary_digest() {
    let workflow = workflow(vec![agent_node("inspect", &[], WorkspaceAccess::ReadOnly)]);
    assert_eq!(
        program_digest(&workflow).unwrap().0,
        "sha256:33e69101a6cbabb61086fad6bad236b9dd703d2b41910f80767c55cdd0a8bdac"
    );
    assert!(canonical_bytes(&workflow).unwrap().starts_with(DOMAIN));
}
