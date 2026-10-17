use pretty_assertions::assert_eq;
use rho_providers::model::Message;
use serde_json::json;

use super::{DisplayRowFormat, StoredDisplayRow};

// Covers: the on-disk row shape. Version 4 rows must stay byte-compatible with
// older binaries; version 5 rows reference equal same-node messages, including
// out-of-order and repeated ones, and keep differing rows inline.
// Owner: display row codec (wire-format contract).
#[test]
fn encodes_rows_in_the_wire_shape_of_each_format() {
    let model = [
        Message::user_text("expanded prompt"),
        Message::assistant_text("ok"),
        Message::assistant_text("done"),
    ];
    let tail = [
        Message::user_text("typed prompt"),
        Message::assistant_text("done"),
        Message::assistant_text("ok"),
        Message::assistant_text("ok"),
    ];

    let rows = |format| {
        serde_json::to_value(StoredDisplayRow::encode(&tail, &model, format, "7")).unwrap()
    };

    assert_eq!(
        rows(DisplayRowFormat::Inline),
        json!([
            {"timestamp": "7", "message": {"User": [{"Text": "typed prompt"}]}},
            {"timestamp": "7", "message": {"Assistant": [{"Text": "done"}]}},
            {"timestamp": "7", "message": {"Assistant": [{"Text": "ok"}]}},
            {"timestamp": "7", "message": {"Assistant": [{"Text": "ok"}]}},
        ])
    );
    assert_eq!(
        rows(DisplayRowFormat::ModelRefs),
        json!([
            {"message": {"User": [{"Text": "typed prompt"}]}},
            {"model": 2},
            {"model": 1},
            {"model": 1},
        ])
    );
}

// Covers: a row that cannot resolve must fail the load, never show a wrong or
// missing transcript row. Owner: display row codec.
#[test]
fn rejects_rows_that_cannot_resolve_in_their_format() {
    let model = [Message::assistant_text("only")];
    for (row, format) in [
        (json!({"model": 0}), DisplayRowFormat::Inline),
        (
            json!({"message": {"Assistant": [{"Text": "x"}]}}),
            DisplayRowFormat::Inline,
        ),
        (json!({"model": 1}), DisplayRowFormat::ModelRefs),
    ] {
        let row: StoredDisplayRow = serde_json::from_value(row.clone()).unwrap();
        let result = StoredDisplayRow::resolve(vec![row.clone()], &model, format, "7", "node");
        assert!(result.is_err(), "{row:?} resolved in {format:?}");
    }

    for malformed in [
        json!({}),
        json!({"model": 0, "message": {"Assistant": []}}),
        json!({"model": 0, "timestamp": "7"}),
    ] {
        assert!(
            serde_json::from_value::<StoredDisplayRow>(malformed.clone()).is_err(),
            "{malformed} parsed"
        );
    }
}
