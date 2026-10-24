use agent_client_protocol::schema::v1;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::permission_request;

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
