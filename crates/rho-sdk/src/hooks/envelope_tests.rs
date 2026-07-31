use pretty_assertions::assert_eq;
use serde_json::json;

use crate::{
    hooks::payload::{
        AfterToolUsePayload, BeforeToolUsePayload, HookCapability, HookPathScope, HookPayload,
        HookPolicyOutcome, HookProcessEnvironment, HookTool, HookToolStatus,
    },
    RunId, SessionId,
};

use super::*;

fn identity() -> HookIdentity {
    HookIdentity {
        session_id: Some(SessionId::from_string("session-1").unwrap()),
        parent_session_id: Some(SessionId::from_string("session-parent").unwrap()),
        run_id: Some(RunId::from_string("run-1").unwrap()),
    }
}

fn envelope(event: HookEventKind, payload: HookPayload) -> HookEnvelope {
    HookEnvelopeBuilder::new(event, identity(), Some(std::path::Path::new("/work"))).finish(payload)
}

/// Normalizes the two fields a golden comparison cannot pin: the generated
/// event ID and the wall-clock timestamp.
fn wire_shape(envelope: &HookEnvelope) -> serde_json::Value {
    let mut value = serde_json::to_value(envelope).unwrap();
    let object = value.as_object_mut().unwrap();
    assert!(!object["event_id"].as_str().unwrap().is_empty());
    assert!(object["timestamp_unix_ms"].as_u64().unwrap() > 0);
    object.insert("event_id".into(), json!("<id>"));
    object.insert("timestamp_unix_ms".into(), json!(0));
    value
}

#[test]
fn before_tool_use_wire_shape_is_stable() {
    let payload = HookPayload::BeforeToolUse(BeforeToolUsePayload {
        tool: HookTool::new("bash", Some("call-1".into())),
        capability: HookCapability::ExecuteProcess {
            working_directory: "/work".into(),
            executable: "bash".into(),
            arguments: vec!["-lc".into()],
            shell_command: Some("git push --force".into()),
            environment: HookProcessEnvironment::InheritAll,
        },
        policy: HookPolicyOutcome::RequireApproval,
    });

    assert_eq!(
        wire_shape(&envelope(HookEventKind::BeforeToolUse, payload)),
        json!({
            "schema_version": 1,
            "event": "before_tool_use",
            "event_id": "<id>",
            "timestamp_unix_ms": 0,
            "identity": {
                "session_id": "session-1",
                "parent_session_id": "session-parent",
                "run_id": "run-1",
            },
            "workspace": { "root": "/work" },
            "bounds": { "truncated": false, "fields": [] },
            "payload": {
                "tool": { "name": "bash", "call_id": "call-1" },
                "capability": {
                    "operation": "execute_process",
                    "working_directory": "/work",
                    "executable": "bash",
                    "arguments": ["-lc"],
                    "shell_command": "git push --force",
                    "environment": "inherit_all",
                },
                "policy": "require_approval",
            },
        })
    );
}

#[test]
fn after_tool_use_wire_shape_is_stable() {
    let payload = HookPayload::AfterToolUse(AfterToolUsePayload {
        tool: HookTool::new("edit_file", Some("call-2".into())),
        status: HookToolStatus::Succeeded,
        failure: None,
        duration_ms: Some(42),
    });

    assert_eq!(
        wire_shape(&envelope(HookEventKind::AfterToolUse, payload))["payload"],
        json!({
            "tool": { "name": "edit_file", "call_id": "call-2" },
            "status": "succeeded",
            "failure": null,
            "duration_ms": 42,
        })
    );
}

#[test]
fn run_failed_wire_shape_carries_a_typed_kind() {
    let payload = HookPayload::RunFailed(crate::hooks::RunFailedPayload {
        failure: crate::hooks::HookFailure {
            kind: "provider".into(),
            message: "provider failed: overloaded".into(),
        },
    });

    assert_eq!(
        wire_shape(&envelope(HookEventKind::RunFailed, payload))["payload"],
        json!({
            "failure": { "kind": "provider", "message": "provider failed: overloaded" },
        })
    );
}

#[test]
fn a_workspaceless_runtime_reports_a_null_root() {
    let envelope =
        HookEnvelopeBuilder::new(HookEventKind::SessionStarted, HookIdentity::default(), None)
            .finish(HookPayload::SessionStarted(
                crate::hooks::SessionStartedPayload {
                    model: "scripted/test".into(),
                },
            ));

    let value = wire_shape(&envelope);
    assert_eq!(value["workspace"], json!({ "root": null }));
    assert_eq!(
        value["identity"],
        json!({
            "session_id": null,
            "parent_session_id": null,
            "run_id": null,
        })
    );
}

#[test]
fn truncated_fields_reach_the_handler_in_the_bounds_report() {
    let mut builder =
        HookEnvelopeBuilder::new(HookEventKind::RunFailed, HookIdentity::default(), None);
    builder.truncation().record("payload.failure.message");
    let envelope = builder.finish(HookPayload::RunFailed(crate::hooks::RunFailedPayload {
        failure: crate::hooks::HookFailure {
            kind: "tool".into(),
            message: "cut".into(),
        },
    }));

    assert_eq!(
        wire_shape(&envelope)["bounds"],
        json!({ "truncated": true, "fields": ["payload.failure.message"] })
    );
}

#[test]
fn an_envelope_within_bounds_serializes() {
    let envelope = envelope(
        HookEventKind::SessionCompleted,
        HookPayload::SessionCompleted(crate::hooks::SessionCompletedPayload { runs: 3 }),
    );

    let encoded = envelope
        .to_bounded_json(HookPayloadBounds::default())
        .expect("small envelope fits the default bound");

    let encoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(encoded["event"], json!("session_completed"));
}

#[test]
fn an_oversized_envelope_is_refused_rather_than_silently_shortened() {
    let envelope = envelope(
        HookEventKind::SessionCompleted,
        HookPayload::SessionCompleted(crate::hooks::SessionCompletedPayload { runs: 3 }),
    );

    let error = envelope
        .to_bounded_json(HookPayloadBounds::new(16, 16))
        .expect_err("a 16-byte bound cannot hold an envelope");

    let HookEnvelopeError::TooLarge(error) = error else {
        panic!("a serializable envelope should fail only because of its size")
    };
    assert_eq!(
        (error.event(), error.limit(), error.size() > 16),
        (HookEventKind::SessionCompleted, 16, true)
    );
}

#[test]
fn accessors_report_what_was_built() {
    let envelope = envelope(
        HookEventKind::AfterToolUse,
        HookPayload::AfterToolUse(AfterToolUsePayload {
            tool: HookTool::new("grep", None),
            status: HookToolStatus::Unavailable,
            failure: None,
            duration_ms: None,
        }),
    );

    assert_eq!(envelope.schema_version(), HOOK_SCHEMA_VERSION);
    assert_eq!(envelope.event(), HookEventKind::AfterToolUse);
    assert_eq!(
        envelope.workspace_root(),
        Some(std::path::Path::new("/work"))
    );
    assert_eq!(envelope.identity(), &identity());
    assert!(!envelope.truncation().is_truncated());
    assert_eq!(envelope.payload().tool_name(), Some("grep"));
    assert!(!envelope.event_id().as_str().is_empty());
}

#[test]
fn path_scopes_serialize_without_leaking_a_granted_root() {
    assert_eq!(
        serde_json::to_value(HookPathScope::from(&crate::PathScope::GrantedRoot {
            root: "/secret/root".into(),
        }))
        .unwrap(),
        json!("granted_root")
    );
}
