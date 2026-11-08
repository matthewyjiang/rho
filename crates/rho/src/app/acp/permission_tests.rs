use std::sync::atomic::AtomicU64;

use agent_client_protocol::schema::v1::{
    PermissionOptionKind, RequestPermissionOutcome, SelectedPermissionOutcome, SessionId, ToolKind,
};
use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, ApprovalAsk, ApprovalSource, InputAnswer, Operation, PathScope};

use super::{
    approval_answer, mode_id, next_placeholder_tool_call_id, parse_mode_id, permission_request,
    plan_answer,
};
use crate::permission::PermissionMode;

fn session_id() -> SessionId {
    SessionId::new("session-1")
}

fn write_ask(reason: &str) -> ApprovalAsk {
    ApprovalAsk {
        source: ApprovalSource::BuiltInTool {
            name: "write".into(),
        },
        operation: Operation::WritePath {
            path: "/workspace/file.rs".into(),
            scope: PathScope::Workspace,
        },
        reason: reason.into(),
    }
}

fn selected(option: &str) -> RequestPermissionOutcome {
    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option.to_owned()))
}

// Covers: hosts must see the three Rho-backed option ids and kinds, on the
// card of the call that asked, or on a placeholder card that takes its kind
// from the operation
// Owner: acp permission mapper
#[test]
fn permission_request_offers_three_options_on_the_asking_card() {
    let placeholder = permission_request(&session_id(), &write_ask("edit file"), None, "pending-1");
    let options = placeholder
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
    let card = |request: &agent_client_protocol::schema::v1::RequestPermissionRequest| {
        (
            request.tool_call.tool_call_id.0.to_string(),
            request.tool_call.fields.kind,
        )
    };
    assert_eq!(
        card(&placeholder),
        ("pending-1".to_owned(), Some(ToolKind::Edit))
    );
    let known = permission_request(
        &session_id(),
        &write_ask("edit file"),
        Some(&wire::ToolCallId::new("call-7")),
        "pending-2",
    );
    assert_eq!(card(&known), ("call-7".to_owned(), None));
}

// Covers: two approval prompts without a tool-call id must not share one ACP id
// Owner: acp permission mapper
#[test]
fn missing_tool_call_ids_do_not_reuse_a_placeholder() {
    let counter = AtomicU64::new(0);
    assert_ne!(
        next_placeholder_tool_call_id(&counter),
        next_placeholder_tool_call_id(&counter)
    );
}

// Covers: ACP outcomes must map onto the daemon's answers; a cancelled prompt
// answers nothing, since the turn's cancel withdraws the request
// Owner: acp permission mapper
#[test]
fn outcomes_map_to_daemon_answers() {
    let cases = [
        ("cancelled", RequestPermissionOutcome::Cancelled, None),
        (
            "allow once",
            selected("allow_once"),
            Some(InputAnswer::AllowOnce),
        ),
        (
            "allow always",
            selected("allow_always"),
            Some(InputAnswer::AllowForSession),
        ),
        (
            "reject",
            selected("reject_once"),
            Some(InputAnswer::Deny { reason: None }),
        ),
        (
            "unknown option",
            selected("reject_always"),
            Some(InputAnswer::Deny {
                reason: Some("unknown permission option".into()),
            }),
        ),
    ];
    for (label, outcome, expected) in cases {
        assert_eq!(approval_answer(&outcome), expected, "{label}");
    }
}

// Covers: a picked mode approves the plan into it; anything else keeps
// planning, and a cancelled prompt answers nothing
// Owner: acp permission mapper
#[test]
fn plan_outcomes_approve_a_mode_or_keep_planning() {
    let cases = [
        ("cancelled", RequestPermissionOutcome::Cancelled, None),
        (
            "mode",
            selected("auto"),
            Some(InputAnswer::ApprovePlan {
                mode: wire::PermissionMode::Auto,
            }),
        ),
        (
            "keep planning",
            selected("keep_planning"),
            Some(InputAnswer::KeepPlanning),
        ),
    ];
    for (label, outcome, expected) in cases {
        assert_eq!(plan_answer(&outcome), expected, "{label}");
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
