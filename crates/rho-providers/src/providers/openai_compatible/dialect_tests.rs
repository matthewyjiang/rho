use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn moonshot_parameters_keep_required_root_object_type() {
    let mut schema = json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "anyOf": [
            {"type": "object", "required": ["path"]},
            {"type": "object", "required": ["edits"]}
        ]
    });

    normalize_moonshot_parameters(&mut schema);

    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {"path": {"type": "string"}}
        })
    );
}

#[test]
fn moonshot_moves_parent_type_into_any_of_branches() {
    let mut schema = json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "anyOf": [
            {"type": "object", "required": ["path"]},
            {"required": ["edits"]}
        ]
    });

    normalize_moonshot_schema(&mut schema);

    assert_eq!(schema.get("type"), None);
    assert_eq!(schema["anyOf"][0]["type"], "object");
    assert_eq!(schema["anyOf"][1]["type"], "object");
    assert_eq!(schema["properties"]["path"]["type"], "string");
}
