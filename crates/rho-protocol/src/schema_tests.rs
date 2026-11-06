use std::collections::BTreeSet;

use pretty_assertions::assert_eq;
use schemars::generate::SchemaSettings;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::OTHER;
use crate::methods::{
    ContentBlock, DetachReason, JobKind, JobState, PlanStatus, SessionEvent, ToolContent, ToolKind,
    ToolStatus,
};

// Covers: the committed schema/protocol.json is what the wire types
// generate, so clients that generate types from it speak the daemon's wire.
// Owner: protocol schema.
#[test]
fn the_committed_schema_matches_the_wire_types() {
    let committed: Value = serde_json::from_str(include_str!("../schema/protocol.json"))
        .expect("schema/protocol.json is JSON");
    assert_eq!(
        committed,
        super::document(),
        "schema/protocol.json is stale; regenerate it with `cargo run -p rho-protocol --example protocol_schema > crates/rho-protocol/schema/protocol.json`"
    );
}

// Covers: one schema describes both what the daemon accepts and what it
// sends, as for an `Option` field serde writes as `null` but reads when
// absent; give such a field `#[serde(default, skip_serializing_if =
// "Option::is_none")]`. Owner: protocol schema.
#[test]
fn every_wire_type_has_one_shape_in_both_directions() {
    assert_eq!(
        super::generate(SchemaSettings::draft2020_12().for_serialize()),
        super::document()
    );
}

// Covers: the enums the schema marks open read every value they do not know
// as `Unknown`, so clients generated from the schema and Rust clients accept
// the same values. Owner: protocol schema.
#[test]
fn enums_marked_open_read_unknown_values_as_unknown() {
    fn reads_as<T: DeserializeOwned + PartialEq>(value: Value, unknown: T) -> bool {
        serde_json::from_value::<T>(value).ok() == Some(unknown)
    }
    let unseen = || json!("from_a_newer_daemon");
    let unseen_tag = || json!({ "type": "from_a_newer_daemon" });
    let cases: [(&str, bool); 9] = [
        ("JobKind", reads_as(unseen(), JobKind::Unknown)),
        ("JobState", reads_as(unseen(), JobState::Unknown)),
        ("ToolKind", reads_as(unseen(), ToolKind::Unknown)),
        ("ToolStatus", reads_as(unseen(), ToolStatus::Unknown)),
        ("PlanStatus", reads_as(unseen(), PlanStatus::Unknown)),
        ("DetachReason", reads_as(unseen(), DetachReason::Unknown)),
        (
            "SessionEvent",
            reads_as(unseen_tag(), SessionEvent::Unknown),
        ),
        ("ToolContent", reads_as(unseen_tag(), ToolContent::Unknown)),
        (
            "ContentBlock",
            reads_as(unseen_tag(), ContentBlock::Unknown),
        ),
    ];

    let document = super::document();
    let open: BTreeSet<&str> = document["$defs"]
        .as_object()
        .expect("$defs is an object")
        .iter()
        .filter(|(_, schema)| {
            schema["anyOf"]
                .as_array()
                .is_some_and(|variants| variants.iter().any(|variant| variant.get(OTHER).is_some()))
        })
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(open, cases.iter().map(|(name, _)| *name).collect());
    for (name, reads_unknown) in cases {
        assert!(
            reads_unknown,
            "{name} does not read an unseen value as Unknown"
        );
    }
}
