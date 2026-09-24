use pretty_assertions::assert_eq;
use serde_json::json;

use super::{JsonScan, StreamedInputJson, EAGER_PARSE_CHARS};

// Covers: string contents, escapes, and nested values must not look like a
// top-level field boundary, or large bodies would re-parse on every fragment
// Owner: claude stream input_json scanner
#[test]
fn scan_reports_only_top_level_field_boundaries() {
    let cases = [
        (r#"{"a":"x,}y""#, false),
        (r#"{"a":"q\"," "#, false),
        (r#"{"a":"q\\","#, true),
        (r#"{"a":[1,2],"#, true),
        (r#"{"a":{"b":1,"c":2}"#, false),
        (r#"{"a":1}"#, true),
        (r#"{"a":1,"#, true),
    ];
    for (text, expected) in cases {
        assert_eq!(JsonScan::default().advance(text), expected, "{text}");
    }
}

// Covers: past the eager window, body fragments skip parsing but the
// fragment closing the object still produces the complete input
// Owner: claude stream input_json assembly
#[test]
fn large_body_parses_at_field_boundaries_only() {
    let body = "x".repeat(EAGER_PARSE_CHARS * 2);
    let mut streamed = StreamedInputJson::default();
    assert_eq!(
        streamed.push(r#"{"file_path":"a.rs""#),
        Some(json!({"file_path": "a.rs"}))
    );
    streamed.push(r#","content":""#);
    let mut parsed_mid_body = 0;
    for chunk in body.as_bytes().chunks(100) {
        let fragment = std::str::from_utf8(chunk).expect("ascii chunk");
        if streamed.push(fragment).is_some() && streamed.len() > EAGER_PARSE_CHARS {
            parsed_mid_body += 1;
        }
    }
    assert_eq!(parsed_mid_body, 0);
    assert_eq!(
        streamed.push(r#""}"#),
        Some(json!({"file_path": "a.rs", "content": body}))
    );
}
