use pretty_assertions::assert_eq;
use serde_json::json;

use super::BuildId;

// Covers: two schemas get one build id exactly when they describe one wire.
// A reworded doc comment keeps the id, so builds that speak alike are not
// told they differ; a changed field, also one named `description`, does not.
// Owner: protocol build id (pure).
#[test]
fn the_id_follows_the_wire_and_not_its_documentation() {
    let schema = |doc: &str, field: &str, description_field: &str| {
        BuildId::of_schema(json!({
            "description": doc,
            "properties": {
                "pid": {"description": doc, "type": field},
                "description": {"type": description_field},
            },
        }))
    };
    let base = schema("The daemon.", "integer", "string");

    assert_eq!(
        [
            schema("The running daemon.", "integer", "string") == base,
            schema("The daemon.", "string", "string") == base,
            schema("The daemon.", "integer", "integer") == base,
        ],
        [true, false, false]
    );
}
