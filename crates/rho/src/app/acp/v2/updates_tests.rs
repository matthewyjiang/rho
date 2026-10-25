use agent_client_protocol::schema::v1;
use pretty_assertions::assert_eq;
use serde_json::{json, Value};

use super::notification;

fn v2_json(update: v1::SessionUpdate) -> Option<Value> {
    notification(v1::SessionNotification::new("s", update))
        .unwrap()
        .map(|notification| serde_json::to_value(notification.update).unwrap())
}

// Covers: v2 redesigned these update shapes. A v1 field Rho leaves out means
// "unchanged", which must stay undefined in v2 rather than become a `null`
// that clears the card; the todo plan needs a plan id; command hints need
// their `text` tag; v2 has no mode update.
// Owner: ACP v2 boundary, v1 -> v2 update mapping
#[test]
fn v1_updates_take_their_v2_shapes() {
    let cases = [
        (
            "status-only tool update keeps the rest of the card",
            v1::SessionUpdate::ToolCallUpdate(v1::ToolCallUpdate::new(
                "call-1",
                v1::ToolCallUpdateFields::new().status(v1::ToolCallStatus::Completed),
            )),
            Some(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-1",
                "status": "completed",
            })),
        ),
        (
            "a new tool call is its first update",
            v1::SessionUpdate::ToolCall(
                v1::ToolCall::new("call-2", "Read notes.txt")
                    .kind(v1::ToolKind::Read)
                    .status(v1::ToolCallStatus::Pending),
            ),
            Some(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-2",
                "title": "Read notes.txt",
                "kind": "read",
                "status": "pending",
                "content": [],
                "locations": [],
            })),
        ),
        (
            "todo plan",
            v1::SessionUpdate::Plan(v1::Plan::new(vec![v1::PlanEntry::new(
                "write tests",
                v1::PlanEntryPriority::Medium,
                v1::PlanEntryStatus::InProgress,
            )])),
            Some(json!({
                "sessionUpdate": "plan_update",
                "plan": {
                    "type": "items",
                    "planId": "rho-todos",
                    "entries": [
                        {"content": "write tests", "priority": "medium", "status": "in_progress"},
                    ],
                },
            })),
        ),
        (
            "command input hint",
            v1::SessionUpdate::AvailableCommandsUpdate(v1::AvailableCommandsUpdate::new(vec![
                v1::AvailableCommand::new("compact", "Compact the context").input(
                    v1::AvailableCommandInput::Unstructured(v1::UnstructuredCommandInput::new(
                        "instructions",
                    )),
                ),
            ])),
            Some(json!({
                "sessionUpdate": "available_commands_update",
                "availableCommands": [{
                    "name": "compact",
                    "description": "Compact the context",
                    "input": {"type": "text", "hint": "instructions"},
                }],
            })),
        ),
        (
            "mode changes travel as config options",
            v1::SessionUpdate::CurrentModeUpdate(v1::CurrentModeUpdate::new("auto")),
            None,
        ),
    ];
    for (label, update, expected) in cases {
        assert_eq!(v2_json(update), expected, "{label}");
    }
}

// Covers: v1 select groups and option ids are renamed in v2 (`groupId`,
// `configId`); a host keyed on the new names would otherwise drop the model
// picker.
// Owner: ACP v2 boundary, v1 -> v2 update mapping
#[test]
fn config_options_use_v2_ids() {
    let option = v1::SessionConfigOption::select(
        "model",
        "Model",
        "grok",
        vec![v1::SessionConfigSelectGroup::new(
            "xai",
            "xAI",
            vec![v1::SessionConfigSelectOption::new("grok", "Grok")],
        )],
    )
    .category(v1::SessionConfigOptionCategory::Model);
    let update = v1::SessionUpdate::ConfigOptionUpdate(v1::ConfigOptionUpdate::new(vec![option]));
    assert_eq!(
        v2_json(update),
        Some(json!({
            "sessionUpdate": "config_option_update",
            "configOptions": [{
                "configId": "model",
                "name": "Model",
                "category": "model",
                "type": "select",
                "currentValue": "grok",
                "options": [{
                    "groupId": "xai",
                    "name": "xAI",
                    "options": [{"value": "grok", "name": "Grok"}],
                }],
            }],
        }))
    );
}

// Covers: v1 shows a deleted file as an emptied one; v2 must tell them apart
// (`delete` vs `modify`) and keep Rho's internal marker off the wire.
// Owner: ACP v2 boundary, v1 -> v2 update mapping
// Unix paths: `/work/...` is not absolute on Windows, which gets no patch.
#[cfg(unix)]
#[test]
fn diffs_tell_deleted_files_from_emptied_ones() {
    let cases = [
        (
            "a deleted file is a delete change, without Rho's marker",
            v1::SessionUpdate::ToolCallUpdate(v1::ToolCallUpdate::new(
                "call-3",
                v1::ToolCallUpdateFields::new().content(vec![v1::ToolCallContent::Diff(
                    v1::Diff::new("/work/old.txt", "")
                        .old_text("bye\n".to_owned())
                        .meta(serde_json::Map::from_iter([(
                            "rho/fileDeleted".to_owned(),
                            json!(true),
                        )])),
                )]),
            )),
            Some(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-3",
                "content": [{
                    "type": "diff",
                    "changes": [{"operation": "delete", "path": "/work/old.txt", "fileType": "text"}],
                    "patch": {
                        "format": "git_patch",
                        "text": "diff --git /work/old.txt /work/old.txt\ndeleted file mode 100644\n--- /work/old.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n",
                    },
                }],
            })),
        ),
        (
            "an emptied file stays a modify",
            v1::SessionUpdate::ToolCallUpdate(v1::ToolCallUpdate::new(
                "call-4",
                v1::ToolCallUpdateFields::new().content(vec![v1::ToolCallContent::Diff(
                    v1::Diff::new("/work/old.txt", "").old_text("bye\n".to_owned()),
                )]),
            )),
            Some(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "call-4",
                "content": [{
                    "type": "diff",
                    "changes": [{"operation": "modify", "path": "/work/old.txt", "fileType": "text"}],
                    "patch": {
                        "format": "git_patch",
                        "text": "diff --git /work/old.txt /work/old.txt\n--- /work/old.txt\n+++ /work/old.txt\n@@ -1 +0,0 @@\n-bye\n",
                    },
                }],
            })),
        ),
    ];
    for (label, update, expected) in cases {
        assert_eq!(v2_json(update), expected, "{label}");
    }
}
