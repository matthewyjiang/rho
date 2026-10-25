use agent_client_protocol::schema::{v1, v2, ProtocolVersion};
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

// Covers: v2 makes `notice` baseline, but T3 Code's v2 client drops it as an
// unknown update, so T3 Code must keep getting notices as agent messages.
// Owner: ACP v2 boundary, initialize mapping
#[test]
fn notices_are_on_for_v2_hosts_except_t3_code() {
    for (client, notices) in [("zed", true), ("t3-code", false)] {
        let request = v2::InitializeRequest::new(
            ProtocolVersion::V2,
            v2::Implementation::new(client, "1.0.0"),
        );
        let session = initialize_request(&request)
            .unwrap()
            .client_capabilities
            .session
            .expect("v2 baseline session features");
        assert_eq!(session.notices.is_some(), notices, "{client}");
    }
}
