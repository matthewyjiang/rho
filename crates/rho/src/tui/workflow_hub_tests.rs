use super::{hub_picker, test_source};
use pretty_assertions::assert_eq;

#[test]
fn hub_picker_shows_start_and_runs_sections() {
    let sources = vec![test_source("review", ".rho/workflows/review/workflow.star")];
    let picker = hub_picker(&sources, &[], &[]);
    assert_eq!(picker.title, "Workflows");
    assert!(picker
        .items
        .iter()
        .any(|item| item.section.as_deref() == Some("Start")));
    assert!(picker
        .items
        .iter()
        .any(|item| item.section.as_deref() == Some("Runs")));
    assert_eq!(picker.items[0].label, "review");
    assert!(picker.items[0].value.starts_with("source:"));
    assert_eq!(picker.items[0].selection_verb, Some("start"));
}

#[test]
fn hub_picker_marks_empty_start_when_no_sources() {
    let picker = hub_picker(&[], &[], &[]);
    assert_eq!(picker.items[0].label, "No local workflows yet");
    assert_eq!(picker.items[0].value, "noop:empty_sources");
}
