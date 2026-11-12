use pretty_assertions::assert_eq;

use super::*;
use crate::model::ToolResult;

// Covers: 5.x history (`ok: bool` results, images in attributed user messages)
// loads as results carrying their images; human uploads and orphans stay put.
// Owner: SDK read-side history conversion.
#[test]
fn legacy_history_moves_tool_images_onto_results() {
    let image = ImageContent {
        data: "aW1hZ2U=".into(),
        mime_type: "image/png".into(),
    };
    let supplement = |call_id: &str| {
        serde_json::json!({"User": [
            {"Text": format!(
                "{PREFIX}{{\"tool_name\":\"capture\",\"tool_call_id\":\"{call_id}\"}}{SUFFIX}"
            )},
            {"Image": {"data": "aW1hZ2U=", "mime_type": "image/png"}},
        ]})
    };
    let upload = serde_json::json!({"User": [
        {"Text": "describe this"},
        {"Image": {"data": "aW1hZ2U=", "mime_type": "image/png"}},
    ]});
    let saved = serde_json::json!([
        {"ToolResult": {"id": "a", "ok": true, "content": "captured"}},
        {"ToolResult": {"id": "b", "ok": false, "content": "failed"}},
        supplement("a"),
        supplement("missing"),
        upload.clone(),
    ]);
    let mut history: Vec<Message> = serde_json::from_value(saved).unwrap();
    attach_legacy_tool_images(&mut history, |message| message);
    let converted = history.clone();
    attach_legacy_tool_images(&mut history, |message| message);

    assert_eq!(history, converted, "conversion is idempotent");
    assert_eq!(
        history,
        vec![
            Message::ToolResult(
                ToolResult::succeeded("a", "captured")
                    .with_images(vec![image])
                    .into()
            ),
            Message::ToolResult(ToolResult::failed("b", "failed").into()),
            serde_json::from_value(supplement("missing")).unwrap(),
            serde_json::from_value(upload).unwrap(),
        ]
    );
}
