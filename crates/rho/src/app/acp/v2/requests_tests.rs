use agent_client_protocol::{schema::ProtocolVersion, v1::schema as v1, v2::schema as v2};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::{initialize_request, permission_request};

// Covers: v1 Rho carries the approval's copy in the tool call title. In v2
// the subject is an upsert of the card, so leaving the copy there would
// rename the tool's card; it must move to the request's own title.
// Owner: ACP v2 boundary, permission request mapping
#[test]
fn permission_copy_moves_off_the_tool_card() {
    let request = v1::RequestPermissionRequest::new(
        "s",
        v1::ToolCallUpdate::new(
            "call-1",
            v1::ToolCallUpdateFields::new().title("Allow writing outside the workspace?"),
        ),
        vec![v1::PermissionOption::new(
            "allow-once",
            "Allow once",
            v1::PermissionOptionKind::AllowOnce,
        )],
    );
    assert_eq!(
        serde_json::to_value(permission_request(request).unwrap()).unwrap(),
        json!({
            "sessionId": "s",
            "title": "Allow writing outside the workspace?",
            "subject": {"type": "tool_call", "toolCall": {"toolCallId": "call-1"}},
            "options": [{"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"}],
        })
    );
}

// Covers: notices are baseline on v2, including T3 Code, without an opt-in
// capability. Falling back to agent messages pollutes the conversation.
// Owner: ACP v2 boundary, initialize mapping
#[test]
fn v2_notices_need_no_host_opt_in() {
    for client in ["zed", "t3-code"] {
        let request = v2::InitializeRequest::new(
            ProtocolVersion::V2,
            v2::Implementation::new(client, "1.0.0"),
        );
        let session = initialize_request(&request)
            .unwrap()
            .client_capabilities
            .session
            .expect("v2 baseline session features");
        assert_eq!(
            session.notices,
            Some(v1::NoticeCapabilities::new()),
            "{client}"
        );
    }
}
