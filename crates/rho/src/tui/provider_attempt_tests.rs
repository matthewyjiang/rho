use super::*;
use crate::tui::{ReasoningEntry, ToolEntry, ToolEntryState};
use std::time::Duration;

#[test]
fn retry_removes_only_replaceable_provider_output() {
    let mut attempt = ProviderAttempt::default();
    let mut transcript = vec![Entry::User("prompt".into())];
    attempt.begin(transcript.len());
    transcript.extend([
        Entry::Assistant("discard assistant".into()),
        Entry::Notice("keep notice".into()),
        Entry::Tool(ToolEntry {
            state: ToolEntryState::Running,
            display_lines: vec!["keep tool".into()],
            expanded: false,
            image: None,
        }),
        Entry::Reasoning(ReasoningEntry {
            text: "discard reasoning".into(),
            thought_for: Some(Duration::from_millis(1_200)),
        }),
    ]);

    assert_eq!(attempt.reset_output(&mut transcript), Some(1));
    assert!(matches!(
        transcript.as_slice(),
        [Entry::User(prompt), Entry::Notice(notice), Entry::Tool(tool)]
            if prompt == "prompt"
                && notice == "keep notice"
                && tool.display_lines == ["keep tool"]
    ));
}

#[test]
fn retry_advances_attempt_boundary_after_cleanup() {
    let mut attempt = ProviderAttempt::default();
    let mut transcript = vec![Entry::User("prompt".into())];
    attempt.begin(transcript.len());
    transcript.push(Entry::Assistant("first attempt".into()));
    attempt.reset_output(&mut transcript);
    transcript.push(Entry::Assistant("second attempt".into()));

    assert_eq!(attempt.reset_output(&mut transcript), Some(1));
    assert!(matches!(transcript.as_slice(), [Entry::User(prompt)] if prompt == "prompt"));
}
