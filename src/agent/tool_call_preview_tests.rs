use super::parse_partial_json;

#[test]
fn completes_streaming_strings_and_nested_values() {
    assert_eq!(
        parse_partial_json(r#"{"path":"src/main"#),
        Some(serde_json::json!({"path": "src/main"}))
    );
    assert_eq!(
        parse_partial_json(r#"{"edits":[{"path":"src/main.rs","new_string":"hel"#),
        Some(serde_json::json!({
            "edits": [{"path": "src/main.rs", "new_string": "hel"}]
        }))
    );
}

#[test]
fn keeps_complete_fields_when_the_next_value_has_not_started() {
    assert_eq!(
        parse_partial_json(r#"{"path":"src/main.rs","offset":"#),
        Some(serde_json::json!({"path": "src/main.rs"}))
    );
}

#[test]
fn preserves_partial_escape_sequences() {
    assert_eq!(
        parse_partial_json(r#"{"content":"line\nnext\\"#),
        Some(serde_json::json!({"content": "line\nnext\\"}))
    );
}
