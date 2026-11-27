use pretty_assertions::assert_eq;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;

use super::*;

/// Asserts `value` encodes as `wire` and `wire` decodes back to `value`.
fn round_trips<T>(value: T, wire: serde_json::Value)
where
    T: Serialize + DeserializeOwned + PartialEq + fmt::Debug,
{
    assert_eq!(serde_json::to_value(&value).unwrap(), wire);
    assert_eq!(serde_json::from_value::<T>(wire).unwrap(), value);
}

// Covers: an import's outcomes, a status, and a revert's params and
// outcomes travel in their wire form, tagged by `type`, with an empty
// `overwrite` left out and read back as empty. Reading unknown values is
// `schema_tests`'s.
// Owner: protocol wire contract.
#[test]
fn sync_journal_messages_travel_in_their_wire_form() {
    round_trips(
        SyncImportKeysResponse::new(vec![
            ImportedSecret::new(SecretItem::api_key("a"), ImportOutcome::Saved),
            ImportedSecret::new(
                SecretItem::api_key("c"),
                ImportOutcome::Failed {
                    message: "no".into(),
                },
            ),
        ]),
        json!({"items": [
            {"item": {"type": "api_key", "auth": "a"}, "outcome": {"type": "saved"}},
            {"item": {"type": "api_key", "auth": "c"}, "outcome": {"type": "failed", "message": "no"}},
        ]}),
    );
    round_trips(
        SyncStatusResponse::new(
            7,
            vec![
                SyncItemStatus::new(SyncItem::api_key("a"), SyncItemState::Unchanged),
                SyncItemStatus::new(SyncItem::api_key("b"), SyncItemState::Changed),
            ],
        ),
        json!({"expires_at": 7, "items": [
            {"item": {"type": "api_key", "auth": "a"}, "state": "unchanged"},
            {"item": {"type": "api_key", "auth": "b"}, "state": "changed"},
        ]}),
    );
    round_trips(
        SyncRevertParams::new(SyncId::new("s"), vec![SyncItem::api_key("a")]),
        json!({"sync": "s", "items": [{"type": "api_key", "auth": "a"}]}),
    );
    round_trips(
        SyncRevertParams::new(SyncId::new("s"), vec![SyncItem::api_key("a")])
            .with_overwrite(vec![SyncItem::api_key("a")]),
        json!({
            "sync": "s",
            "items": [{"type": "api_key", "auth": "a"}],
            "overwrite": [{"type": "api_key", "auth": "a"}],
        }),
    );
    round_trips(
        SyncRevertResponse::new(vec![
            RevertedItem::new(SyncItem::api_key("a"), RevertOutcome::Restored),
            RevertedItem::new(SyncItem::api_key("b"), RevertOutcome::Changed),
            RevertedItem::new(
                SyncItem::api_key("c"),
                RevertOutcome::Failed {
                    message: "no".into(),
                },
            ),
        ]),
        json!({"items": [
            {"item": {"type": "api_key", "auth": "a"}, "outcome": {"type": "restored"}},
            {"item": {"type": "api_key", "auth": "b"}, "outcome": {"type": "changed"}},
            {"item": {"type": "api_key", "auth": "c"}, "outcome": {"type": "failed", "message": "no"}},
        ]}),
    );
}
