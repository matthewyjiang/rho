use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

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

#[test]
fn moonshot_serializes_enabled_and_disabled_thinking() {
    assert_eq!(
        OpenAiCompatibleDialect::Moonshot.thinking("kimi-code", "k3", ReasoningLevel::Off,),
        Some(OpenAiThinking { kind: "disabled" })
    );
    assert_eq!(
        OpenAiCompatibleDialect::Moonshot.thinking("moonshot", "kimi-k3", ReasoningLevel::Max,),
        Some(OpenAiThinking { kind: "enabled" })
    );
    assert_eq!(
        OpenAiCompatibleDialect::Moonshot.thinking("moonshot", "kimi-k2.5", ReasoningLevel::Max,),
        None
    );
}
