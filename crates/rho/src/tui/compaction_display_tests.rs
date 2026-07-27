use pretty_assertions::assert_eq;

use super::*;

#[test]
fn running_lines_are_minimal() {
    assert_eq!(
        running_card().to_display_lines(),
        vec![
            "● compact".to_string(),
            "  └ shrinking context…".to_string(),
        ]
    );
}

#[test]
fn completed_lines_prefer_tokens_then_messages() {
    let lines = completed_card(CompactionDisplayFacts {
        previous_messages: 48,
        current_messages: 4,
        previous_tokens: 12_400,
        current_tokens: 3_100,
        cost_usd_micros: None,
    })
    .to_display_lines();
    assert_eq!(
        lines,
        vec![
            "✓ compact".to_string(),
            "  ├ 12.4K → 3.1K tokens  (−9.3K · 75%)".to_string(),
            "  └ 48 → 4 messages  (−44)".to_string(),
        ]
    );
}

#[test]
fn completed_lines_include_cost_when_present() {
    let lines = completed_card(CompactionDisplayFacts {
        previous_messages: 10,
        current_messages: 3,
        previous_tokens: 2_000,
        current_tokens: 500,
        cost_usd_micros: Some(4_200),
    })
    .to_display_lines();
    assert!(lines.iter().any(|line| line.contains("cost $0.004")));
}

#[test]
fn completed_lines_message_only_when_tokens_missing() {
    let lines = completed_card(CompactionDisplayFacts {
        previous_messages: 12,
        current_messages: 4,
        previous_tokens: 0,
        current_tokens: 0,
        cost_usd_micros: None,
    })
    .to_display_lines();
    assert_eq!(
        lines,
        vec![
            "✓ compact".to_string(),
            "  └ 12 → 4 messages  (−8)".to_string(),
        ]
    );
}

#[test]
fn completed_lines_mark_no_token_change() {
    let lines = completed_card(CompactionDisplayFacts {
        previous_messages: 5,
        current_messages: 5,
        previous_tokens: 1_000,
        current_tokens: 1_000,
        cost_usd_micros: None,
    })
    .to_display_lines();
    assert!(lines.iter().any(|line| line.contains("(no change)")));
}

#[test]
fn failed_unchanged_and_cancelled_lines() {
    assert_eq!(
        failed_card("provider unavailable").to_display_lines(),
        vec![
            "✗ compact".to_string(),
            "  ├ failed".to_string(),
            "  └ provider unavailable".to_string(),
        ]
    );
    assert_eq!(
        unchanged_card("not enough conversation history to compact").to_display_lines(),
        vec![
            "✓ compact".to_string(),
            "  └ not enough conversation history to compact".to_string(),
        ]
    );
    assert_eq!(
        cancelled_card().to_display_lines(),
        vec!["■ compact".to_string(), "  └ cancelled".to_string(),]
    );
    assert!(CompactionUiOutcome::Unchanged {
        detail: "noop".into()
    }
    .ok());
    assert!(!CompactionUiOutcome::Failed {
        detail: "boom".into()
    }
    .ok());
    assert!(CompactionUiOutcome::Cancelled.ok());
}
