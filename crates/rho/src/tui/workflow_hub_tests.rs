use super::{hub_picker, test_source};
use pretty_assertions::assert_eq;

#[test]
fn hub_picker_labels_say_what_enter_does() {
    let sources = vec![test_source("review", ".rho/workflows/review/workflow.star")];
    let picker = hub_picker(&sources, &[], &[]);
    assert_eq!(picker.title, "Workflows");
    assert!(picker.is_overlay());
    assert!(picker
        .items
        .iter()
        .any(|item| item.section.as_deref() == Some("START")));
    assert!(picker
        .items
        .iter()
        .any(|item| item.section.as_deref() == Some("RUNS")));
    let start = picker
        .items
        .iter()
        .find(|item| item.value.starts_with("source:"))
        .expect("start row");
    assert_eq!(start.label, "Start  review");
    assert_eq!(start.selection_verb, Some("start"));
    assert!(start
        .badge
        .as_ref()
        .is_some_and(|badge| badge.text == "new run"));
}

#[test]
fn hub_picker_marks_empty_start_when_no_sources() {
    let picker = hub_picker(&[], &[], &[]);
    assert_eq!(picker.items[0].label, "No local workflows yet");
    assert_eq!(picker.items[0].value, "noop:empty_sources");
    assert_eq!(picker.items[0].section.as_deref(), Some("START"));
}
