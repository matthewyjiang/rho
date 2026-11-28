use pretty_assertions::assert_eq;
use serde_json::json;

use super::{revision, to_item, to_json};
use crate::config::ConfigDocument;

// Covers: an entry copied as JSON keeps every field and its TOML type
// (integers stay integers, nested tables stay tables, empty ones too),
// whichever form the file wrote it in, and through the text it saves as.
// Owner: config table entries.
#[test]
fn an_entry_keeps_its_fields_and_types_through_json() {
    let doc = ConfigDocument::parse(
        r#"
[entry]
url = "http://127.0.0.1:8080"
retries = 3
ratio = 0.5
enabled = true
args = ["--fast", "--quiet"]
inline = { name = "x" }
empty = {}

[entry.nested]
depth = 2
"#,
    )
    .unwrap();
    let json = to_json(doc.get("entry").unwrap());
    assert_eq!(
        json,
        json!({
            "url": "http://127.0.0.1:8080",
            "retries": 3,
            "ratio": 0.5,
            "enabled": true,
            "args": ["--fast", "--quiet"],
            "inline": {"name": "x"},
            "empty": {},
            "nested": {"depth": 2},
        })
    );
    let mut copied = ConfigDocument::default();
    copied.set_item_at(&["entry"], Some(to_item(&json).unwrap()));
    let saved = ConfigDocument::parse(&copied.text()).unwrap();
    assert_eq!(to_json(saved.get("entry").unwrap()), json);
    assert_eq!(
        to_json(&to_item(&json!("openai/gpt-5.5")).unwrap()),
        json!("openai/gpt-5.5")
    );
}

// Covers: a value TOML cannot hold is refused rather than changed: a null,
// named where it sits, and an integer past TOML's range, which would
// otherwise be saved as a float.
// Owner: config table entries.
#[test]
fn a_value_toml_cannot_hold_is_refused() {
    let number = |text: &str| serde_json::from_str::<serde_json::Value>(text).unwrap();
    let cases = [
        (json!(null), "must not be null"),
        (
            json!({"env": {"TOKEN": null}}),
            "`env.TOKEN` is null, which TOML cannot hold",
        ),
        (
            json!({"args": ["a", null]}),
            "`args.1` is null, which TOML cannot hold",
        ),
        (
            number("18446744073709551615"),
            "18446744073709551615 is past TOML's integer range",
        ),
        (
            number("100000000000000000000000"),
            "100000000000000000000000 is past TOML's integer range",
        ),
        (
            number("-9223372036854775809"),
            "-9223372036854775809 is past TOML's integer range",
        ),
        (number("1e400"), "1e+400 is past TOML's float range"),
    ];
    for (value, message) in cases {
        assert_eq!(
            to_item(&value).map(drop),
            Err(message.to_owned()),
            "{value}"
        );
    }
}

// Covers: a revision follows the entries' content, not the order the file
// wrote their fields in.
// Owner: config table entries.
#[test]
fn a_revision_follows_content_not_field_order() {
    let ordered = json!([{"key": "a", "value": {"x": 1, "y": 2}}]);
    let reordered = json!([{"value": {"y": 2, "x": 1}, "key": "a"}]);
    let changed = json!([{"key": "a", "value": {"x": 1, "y": 3}}]);
    assert_eq!(revision(&ordered), revision(&reordered));
    assert_ne!(revision(&ordered), revision(&changed));
}
