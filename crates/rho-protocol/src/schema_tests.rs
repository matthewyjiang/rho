use std::collections::BTreeSet;

use pretty_assertions::assert_eq;
use schemars::generate::SchemaSettings;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::OTHER;
use crate::display::{
    CodeWindow, DiffRowKind, NotificationDelivery, NotificationPreview, NotificationTone,
    NotificationVisibility, ToolCardStatus, ToolFact, ToolFamily, ToolHeader,
};
use crate::methods::{
    AnswerStop, ApprovalSource, Ask, AuthKind, AuthStatus, Budget, CardTone, CommandKind,
    CompactionStatus, ContentBlock, DefaultStyle, DetachReason, JobKind, JobState, LoginOutcome,
    Operation, PathScope, PermissionMode, PlanStatus, ReasoningLevel, Resolution, Selection,
    SessionEvent, SessionGoalPhase, SessionMcpMode, SessionMcpStatus, ToolContent, ToolKind,
    ToolStatus, TurnErrorKind, TurnOutcome, UncheckedReason, UnsteerOutcome,
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
    let unseen_kind = || json!({ "kind": "from_a_newer_daemon" });
    let cases: [(&str, bool); 43] = [
        (
            "SessionGoalPhase",
            reads_as(unseen(), SessionGoalPhase::Unknown),
        ),
        (
            "SessionMcpMode",
            reads_as(unseen(), SessionMcpMode::Unknown),
        ),
        (
            "SessionMcpStatus",
            reads_as(unseen(), SessionMcpStatus::Unknown),
        ),
        ("JobKind", reads_as(unseen(), JobKind::Unknown)),
        ("JobState", reads_as(unseen(), JobState::Unknown)),
        ("Budget", reads_as(unseen(), Budget::Unknown)),
        ("ToolKind", reads_as(unseen(), ToolKind::Unknown)),
        ("ToolStatus", reads_as(unseen(), ToolStatus::Unknown)),
        ("PlanStatus", reads_as(unseen(), PlanStatus::Unknown)),
        ("DetachReason", reads_as(unseen(), DetachReason::Unknown)),
        (
            "UnsteerOutcome",
            reads_as(unseen(), UnsteerOutcome::Unknown),
        ),
        (
            "PermissionMode",
            reads_as(unseen(), PermissionMode::Unknown),
        ),
        ("Resolution", reads_as(unseen(), Resolution::Unknown)),
        ("AnswerStop", reads_as(unseen(), AnswerStop::Unknown)),
        ("TurnErrorKind", reads_as(unseen(), TurnErrorKind::Unknown)),
        ("TurnOutcome", reads_as(unseen_tag(), TurnOutcome::Unknown)),
        ("Selection", reads_as(unseen(), Selection::Unknown)),
        ("DefaultStyle", reads_as(unseen(), DefaultStyle::Unknown)),
        ("Ask", reads_as(unseen_kind(), Ask::Unknown)),
        (
            "ApprovalSource",
            reads_as(unseen_kind(), ApprovalSource::Unknown),
        ),
        ("Operation", reads_as(unseen_kind(), Operation::Unknown)),
        ("PathScope", reads_as(unseen_kind(), PathScope::Unknown)),
        (
            "SessionEvent",
            reads_as(unseen_tag(), SessionEvent::Unknown),
        ),
        ("ToolContent", reads_as(unseen_tag(), ToolContent::Unknown)),
        (
            "ContentBlock",
            reads_as(unseen_tag(), ContentBlock::Unknown),
        ),
        (
            "CompactionStatus",
            reads_as(unseen_tag(), CompactionStatus::Unknown),
        ),
        ("CardTone", reads_as(unseen(), CardTone::Unknown)),
        (
            "UncheckedReason",
            reads_as(unseen(), UncheckedReason::Unknown),
        ),
        (
            "ReasoningLevel",
            reads_as(unseen(), ReasoningLevel::Unknown),
        ),
        ("CommandKind", reads_as(unseen(), CommandKind::Unknown)),
        ("AuthKind", reads_as(unseen(), AuthKind::Unknown)),
        ("AuthStatus", reads_as(unseen(), AuthStatus::Unknown)),
        (
            "LoginOutcome",
            reads_as(unseen_tag(), LoginOutcome::Unknown),
        ),
        ("ToolFamily", reads_as(unseen(), ToolFamily::Unknown)),
        (
            "ToolCardStatus",
            reads_as(unseen(), ToolCardStatus::Unknown),
        ),
        ("ToolHeader", reads_as(unseen_kind(), ToolHeader::Unknown)),
        ("ToolFact", reads_as(unseen_kind(), ToolFact::Unknown)),
        ("DiffRowKind", reads_as(unseen(), DiffRowKind::Unknown)),
        ("CodeWindow", reads_as(unseen(), CodeWindow::Unknown)),
        (
            "NotificationDelivery",
            reads_as(unseen(), NotificationDelivery::Unknown),
        ),
        (
            "NotificationTone",
            reads_as(unseen(), NotificationTone::Unknown),
        ),
        (
            "NotificationPreview",
            reads_as(unseen(), NotificationPreview::Unknown),
        ),
        (
            "NotificationVisibility",
            reads_as(unseen(), NotificationVisibility::Unknown),
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
    // Marked variants lose their literal `unknown`; one left over is an
    // `Unknown` variant whose enum the schema would publish as closed.
    assert_eq!(
        literal_unknowns(&document["$defs"], "$defs"),
        Vec::<String>::new(),
        "mark each #[serde(other)] variant #[schemars(extend(\"x-rho-other\" = true))]"
    );
}

/// Where under `schema` a value or tag is the literal `"unknown"`.
fn literal_unknowns(schema: &Value, path: &str) -> Vec<String> {
    match schema {
        Value::Object(fields) if fields.get("const") == Some(&json!("unknown")) => {
            vec![path.to_owned()]
        }
        Value::Object(fields) => fields
            .iter()
            .flat_map(|(key, value)| literal_unknowns(value, &format!("{path}/{key}")))
            .collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .flat_map(|(index, value)| literal_unknowns(value, &format!("{path}/{index}")))
            .collect(),
        _ => Vec::new(),
    }
}
