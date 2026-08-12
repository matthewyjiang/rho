use super::*;
use crate::tui::{ReasoningEntry, ToolEntry};
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
            card: rho_tools::tool_card::ToolCard::new(
                rho_tools::tool_card::ToolStatus::Running,
                rho_tools::tool_card::ToolFamily::Default,
                rho_tools::tool_card::ToolHeader::call("keep tool", None),
            ),
            expanded: false,
            image: None,
            started_at: None,
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
                && tool.card.header_text() == "● keep tool"
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
