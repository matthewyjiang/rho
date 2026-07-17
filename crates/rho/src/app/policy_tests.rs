use pretty_assertions::assert_eq;
use rho_sdk::{CapabilityRequest, CapabilitySource, PathScope, PolicyDecision, WorkspacePolicy};

use super::AppPolicy;
use crate::permission::PermissionMode;

fn write_request() -> CapabilityRequest {
    CapabilityRequest::write_path(
        "/workspace/file",
        PathScope::PrimaryWorkspace,
        CapabilitySource::built_in_tool("write_file"),
    )
}

#[test]
fn allow_policy_allows_requests() {
    assert_eq!(
        AppPolicy::for_mode(PermissionMode::Auto).evaluate(&write_request()),
        PolicyDecision::Allow
    );
}

#[test]
fn mode_policy_dispatches_to_permission_mode() {
    let request = write_request();
    for mode in [PermissionMode::Plan, PermissionMode::Supervised] {
        assert_eq!(
            AppPolicy::for_mode(mode).evaluate(&request),
            mode.decision_for(request.kind())
        );
    }
}
