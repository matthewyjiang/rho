use pretty_assertions::assert_eq;

use super::*;

#[test]
fn short_values_are_left_alone() {
    let mut value = "keep".to_owned();
    assert!(!truncate_field(&mut value, HookPayloadBounds::new(16, 64)));
    assert_eq!(value, "keep");
}

#[test]
fn long_values_are_cut_to_the_field_bound() {
    let mut value = "0123456789".to_owned();
    assert!(truncate_field(&mut value, HookPayloadBounds::new(4, 64)));
    assert_eq!(value, "0123");
}

#[test]
fn truncation_never_splits_a_character() {
    // Four bytes of one multi-byte character; a three-byte bound must drop it.
    let mut value = "\u{1F600}tail".to_owned();
    assert!(truncate_field(&mut value, HookPayloadBounds::new(3, 64)));
    assert_eq!(value, "");
}

#[test]
fn bounds_never_go_below_one_byte() {
    let bounds = HookPayloadBounds::new(0, 0);
    assert_eq!(bounds.max_field_bytes(), 1);
    assert_eq!(bounds.max_envelope_bytes(), 1);
}

#[test]
fn a_fresh_report_claims_nothing_was_shortened() {
    let report = HookTruncation::default();
    assert!(!report.is_truncated());
    assert_eq!(report.fields().count(), 0);
}

#[test]
fn recorded_fields_are_sorted_and_deduplicated() {
    let mut report = HookTruncation::default();
    report.record("payload.b");
    report.record("payload.a");
    report.record("payload.b");
    assert!(report.is_truncated());
    assert_eq!(
        report.fields().collect::<Vec<_>>(),
        vec!["payload.a", "payload.b"]
    );
}

#[test]
fn serialized_report_names_the_shortened_fields() {
    let mut report = HookTruncation::default();
    report.record("payload.capability.shell_command");
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        serde_json::json!({
            "truncated": true,
            "fields": ["payload.capability.shell_command"],
        })
    );
}
