use pretty_assertions::assert_eq;

use super::*;

#[test]
fn compact_fixtures_match_seed_prompt_after_empty_user_entry() {
    let messages = vec![
        Message::user_text("fixture compact until cancel"),
        Message::Assistant(vec![ContentBlock::Text("ok".into())]),
        Message::User(vec![]),
    ];
    let request = ModelRequest {
        messages: &messages,
        tools: &[],
        cancellation: CancellationToken::new(),
        reasoning_level: rho_sdk::ReasoningLevel::Medium,
        prompt_cache_key: None,
    };
    assert_eq!(
        last_user_text(&request).as_deref(),
        Some("fixture compact until cancel")
    );
    assert!(compact::native_compact(request).is_some());
}
