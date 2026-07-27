use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolFact, ToolStatus};

use super::*;

fn fact_texts(card: &rho_tools::tool_card::ToolCard) -> Vec<String> {
    card.facts.iter().map(ToolFact::plain_text).collect()
}

#[test]
fn running_card_is_minimal() {
    let card = running_card();
    assert_eq!(card.status, ToolStatus::Running);
    assert_eq!(card.header_text(), "● compact");
    assert_eq!(fact_texts(&card), vec!["shrinking context…".to_string()]);
}

#[test]
fn completed_card_prefers_tokens_then_messages() {
    let card = completed_card(CompactionDisplayFacts {
        previous_messages: 48,
        current_messages: 4,
        previous_tokens: 12_400,
        current_tokens: 3_100,
        cost_usd_micros: None,
    });
    assert_eq!(card.header_text(), "✓ compact");
    assert_eq!(
        fact_texts(&card),
        vec![
            "12.4K → 3.1K tokens  (−9.3K · 75%)".to_string(),
            "48 → 4 messages  (−44)".to_string(),
        ]
    );
}

#[test]
fn completed_card_includes_cost_when_present() {
    let card = completed_card(CompactionDisplayFacts {
        previous_messages: 10,
        current_messages: 3,
        previous_tokens: 2_000,
        current_tokens: 500,
        cost_usd_micros: Some(4_200),
    });
    assert!(fact_texts(&card)
        .iter()
        .any(|line| line.contains("cost $0.004")));
}

#[test]
fn completed_card_message_only_when_tokens_missing() {
    let card = completed_card(CompactionDisplayFacts {
        previous_messages: 12,
        current_messages: 4,
        previous_tokens: 0,
        current_tokens: 0,
        cost_usd_micros: None,
    });
    assert_eq!(card.header_text(), "✓ compact");
    assert_eq!(fact_texts(&card), vec!["12 → 4 messages  (−8)".to_string()]);
}

#[test]
fn completed_card_marks_no_token_change() {
    let card = completed_card(CompactionDisplayFacts {
        previous_messages: 5,
        current_messages: 5,
        previous_tokens: 1_000,
        current_tokens: 1_000,
        cost_usd_micros: None,
    });
    assert!(fact_texts(&card)
        .iter()
        .any(|line| line.contains("(no change)")));
}

#[test]
fn failed_unchanged_and_cancelled_cards() {
    let failed = failed_card("provider unavailable");
    assert_eq!(failed.header_text(), "✗ compact");
    assert_eq!(
        fact_texts(&failed),
        vec!["failed".to_string(), "provider unavailable".to_string()]
    );

    let unchanged = unchanged_card("not enough conversation history to compact");
    assert_eq!(unchanged.header_text(), "✓ compact");
    assert_eq!(
        fact_texts(&unchanged),
        vec!["not enough conversation history to compact".to_string()]
    );

    let cancelled = cancelled_card();
    assert_eq!(cancelled.header_text(), "■ compact");
    assert_eq!(fact_texts(&cancelled), vec!["cancelled".to_string()]);

    assert_eq!(
        CompactionUiOutcome::Unchanged {
            detail: "noop".into()
        }
        .card()
        .status,
        ToolStatus::Ok
    );
    assert_eq!(
        CompactionUiOutcome::Failed {
            detail: "boom".into()
        }
        .card()
        .status,
        ToolStatus::Error
    );
    assert_eq!(
        CompactionUiOutcome::Cancelled.card().status,
        ToolStatus::Interrupted
    );
}
