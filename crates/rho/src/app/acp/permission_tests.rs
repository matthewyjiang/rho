use agent_client_protocol::schema::v1::{
    PermissionOptionKind, RequestPermissionOutcome, SelectedPermissionOutcome, SessionId,
};
use pretty_assertions::assert_eq;
use rho_sdk::{ApprovalDecision, ApprovalRequest, CapabilityRequest, CapabilitySource, PathScope};

use std::sync::atomic::AtomicU64;

use super::{
    decision_for, mode_id, next_placeholder_tool_call_id, parse_mode_id, permission_request,
};
use crate::permission::PermissionMode;

fn session_id() -> SessionId {
    SessionId::new("session-1")
}

fn write_request(reason: &str) -> ApprovalRequest {
    ApprovalRequest::new(
        CapabilityRequest::write_path(
            "/workspace/file.rs",
            PathScope::PrimaryWorkspace,
            CapabilitySource::built_in_tool("write"),
        ),
        reason,
    )
}

// Covers: hosts must see the three Rho-backed option ids and kinds
// Owner: acp permission mapper
#[test]
fn permission_request_offers_three_session_scoped_options() {
    let request = permission_request(&session_id(), &write_request("edit file"), "pending-1");
    let options = request
        .options
        .iter()
        .map(|option| (option.option_id.0.as_ref(), option.kind))
        .collect::<Vec<_>>();
    assert_eq!(
        options,
        [
            ("allow_once", PermissionOptionKind::AllowOnce),
            ("allow_always", PermissionOptionKind::AllowAlways),
            ("reject_once", PermissionOptionKind::RejectOnce),
        ]
    );
    assert_eq!(request.tool_call.tool_call_id.0.as_ref(), "pending-1");
}

// Covers: two approval prompts without a tool-call id must not share one ACP id
// Owner: acp permission mapper
#[test]
fn missing_tool_call_ids_do_not_reuse_a_placeholder() {
    let counter = AtomicU64::new(0);
    let first = permission_request(
        &session_id(),
        &write_request("first"),
        &next_placeholder_tool_call_id(&counter),
    );
    let second = permission_request(
        &session_id(),
        &write_request("second"),
        &next_placeholder_tool_call_id(&counter),
    );
    assert_ne!(
        first.tool_call.tool_call_id.0.as_ref(),
        second.tool_call.tool_call_id.0.as_ref()
    );
}

// Covers: ACP outcomes must collapse onto Rho's session-scoped decisions
// Owner: acp permission mapper
#[test]
fn decision_for_maps_selected_cancelled_and_unknown() {
    let cases = [
        (
            RequestPermissionOutcome::Cancelled,
            ApprovalDecision::Deny {
                reason: "permission request cancelled".into(),
            },
        ),
        (
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("allow_once")),
            ApprovalDecision::AllowOnce,
        ),
        (
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("allow_always")),
            ApprovalDecision::AllowForSession,
        ),
        (
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("reject_once")),
            ApprovalDecision::Deny {
                reason: "permission rejected".into(),
            },
        ),
        (
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("reject_always")),
            ApprovalDecision::Deny {
                reason: "unknown permission option".into(),
            },
        ),
    ];
    for (outcome, expected) in cases {
        assert_eq!(decision_for(&outcome), expected);
    }
}

// Covers: advertised ACP mode ids must round-trip onto PermissionMode
// Owner: acp permission mapper
#[test]
fn mode_ids_round_trip_every_permission_mode() {
    for mode in PermissionMode::ALL {
        assert_eq!(parse_mode_id(mode_id(mode).0.as_ref()), Some(mode));
    }
    assert_eq!(parse_mode_id("unknown"), None);
}
