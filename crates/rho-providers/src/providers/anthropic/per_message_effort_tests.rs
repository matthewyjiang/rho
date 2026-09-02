use pretty_assertions::assert_eq;
use serde_json::json;

use super::{apply, beta_header, PerMessageEffortState, MAX_TRACKED_CONVERSATIONS};
use crate::protocol::anthropic_messages::{
    AnthropicContentBlock, AnthropicMessage, AnthropicOutputConfig, AnthropicRole,
};

const FABLE_5_1: &str = "claude-fable-5-1";
const FABLE_5: &str = "claude-fable-5";

fn effort(name: &'static str) -> AnthropicOutputConfig {
    AnthropicOutputConfig { effort: name }
}

fn user(text: &str) -> AnthropicMessage {
    AnthropicMessage::new(
        AnthropicRole::User,
        vec![AnthropicContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
    )
}

fn assistant(text: &str) -> AnthropicMessage {
    AnthropicMessage::new(
        AnthropicRole::Assistant,
        vec![AnthropicContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
    )
}

fn turn(user_count: usize) -> Vec<AnthropicMessage> {
    let mut messages = Vec::new();
    for index in 1..=user_count {
        if index > 1 {
            messages.push(assistant("ok"));
        }
        messages.push(user(&index.to_string()));
    }
    messages
}

fn system_efforts(messages: &[AnthropicMessage]) -> Vec<&'static str> {
    messages
        .iter()
        .filter_map(|message| {
            (message.role == AnthropicRole::System)
                .then_some(message.output_config.as_ref()?.effort)
        })
        .collect()
}

// Covers: mid-conversation effort changes keep the prefix top-level
// value and leave effort-only system messages at the change points
// Owner: anthropic per-message effort
#[test]
fn conversation_effort_changes_keep_prefix_and_pin_system_messages() {
    let mut state = PerMessageEffortState::default();

    let mut first = turn(1);
    let top = apply(
        FABLE_5_1,
        &mut state,
        Some("rho:a"),
        Some(effort("high")),
        &mut first,
    );
    assert_eq!(top, Some(effort("high")));
    assert!(system_efforts(&first).is_empty());
    assert_eq!(beta_header(&first), None);

    let mut second = turn(2);
    let top = apply(
        FABLE_5_1,
        &mut state,
        Some("rho:a"),
        Some(effort("low")),
        &mut second,
    );
    assert_eq!(top, Some(effort("high")));
    assert_eq!(system_efforts(&second), ["low"]);
    assert_eq!(second[2].role, AnthropicRole::System);
    assert_eq!(second[3].role, AnthropicRole::User);
    assert_eq!(beta_header(&second), Some(super::BETA));
    assert_eq!(
        serde_json::to_value(&second[2]).unwrap(),
        json!({
            "role": "system",
            "content": [],
            "output_config": {"effort": "low"}
        })
    );

    let mut third = turn(3);
    let top = apply(
        FABLE_5_1,
        &mut state,
        Some("rho:a"),
        Some(effort("low")),
        &mut third,
    );
    assert_eq!(top, Some(effort("high")));
    assert_eq!(system_efforts(&third), ["low"]);
    assert_eq!(third[2].role, AnthropicRole::System);

    let mut fourth = turn(4);
    let top = apply(
        FABLE_5_1,
        &mut state,
        Some("rho:a"),
        Some(effort("medium")),
        &mut fourth,
    );
    assert_eq!(top, Some(effort("high")));
    assert_eq!(system_efforts(&fourth), ["low", "medium"]);

    let mut fifth = turn(5);
    let top = apply(
        FABLE_5_1,
        &mut state,
        Some("rho:a"),
        Some(effort("high")),
        &mut fifth,
    );
    assert_eq!(top, Some(effort("high")));
    assert_eq!(system_efforts(&fifth), ["low", "medium", "high"]);
}

// Covers: same-length effort changes, compacted history, Off, and
// unsupported models keep today's top-level effort
// Owner: anthropic per-message effort
#[test]
fn non_continuation_requests_use_top_level_current_effort() {
    let mut state = PerMessageEffortState::default();
    let mut messages = turn(1);
    apply(
        FABLE_5_1,
        &mut state,
        Some("rho:a"),
        Some(effort("low")),
        &mut messages,
    );

    let mut same_len = turn(1);
    let top = apply(
        FABLE_5_1,
        &mut state,
        Some("rho:a"),
        Some(effort("high")),
        &mut same_len,
    );
    assert_eq!(top, Some(effort("high")));
    assert!(system_efforts(&same_len).is_empty());

    let mut continued = turn(2);
    apply(
        FABLE_5_1,
        &mut state,
        Some("rho:a"),
        Some(effort("high")),
        &mut continued,
    );
    let mut compacted = turn(1);
    let top = apply(
        FABLE_5_1,
        &mut state,
        Some("rho:a"),
        Some(effort("medium")),
        &mut compacted,
    );
    assert_eq!(top, Some(effort("medium")));
    assert!(system_efforts(&compacted).is_empty());

    let mut off_messages = turn(2);
    let top = apply(
        FABLE_5_1,
        &mut state,
        Some("rho:a"),
        None,
        &mut off_messages,
    );
    assert_eq!(top, None);
    assert!(system_efforts(&off_messages).is_empty());

    let mut unsupported = turn(1);
    let top = apply(
        FABLE_5,
        &mut state,
        Some("rho:a"),
        Some(effort("low")),
        &mut unsupported,
    );
    let mut unsupported_next = turn(2);
    let top_next = apply(
        FABLE_5,
        &mut state,
        Some("rho:a"),
        Some(effort("high")),
        &mut unsupported_next,
    );
    assert_eq!(top, Some(effort("low")));
    assert_eq!(top_next, Some(effort("high")));
    assert!(system_efforts(&unsupported).is_empty());
    assert!(system_efforts(&unsupported_next).is_empty());
}

// Covers: one provider shared by two sessions must not treat session B's
// longer request as a continuation of session A, nor leak A's prefix effort
// Owner: anthropic per-message effort
#[test]
fn interleaved_conversations_on_one_provider_keep_separate_effort_state() {
    let mut state = PerMessageEffortState::default();

    let mut a1 = turn(1);
    assert_eq!(
        apply(
            FABLE_5_1,
            &mut state,
            Some("rho:a"),
            Some(effort("high")),
            &mut a1
        ),
        Some(effort("high"))
    );

    // B starts longer than A and at a different effort: must be its own prefix.
    let mut b1 = turn(2);
    assert_eq!(
        apply(
            FABLE_5_1,
            &mut state,
            Some("rho:b"),
            Some(effort("low")),
            &mut b1
        ),
        Some(effort("low"))
    );
    assert!(system_efforts(&b1).is_empty());

    // A continues and changes effort: shift lands under A only.
    let mut a2 = turn(2);
    assert_eq!(
        apply(
            FABLE_5_1,
            &mut state,
            Some("rho:a"),
            Some(effort("medium")),
            &mut a2
        ),
        Some(effort("high"))
    );
    assert_eq!(system_efforts(&a2), ["medium"]);

    // B continues at its own level: no A shifts, B's prefix intact.
    let mut b2 = turn(3);
    assert_eq!(
        apply(
            FABLE_5_1,
            &mut state,
            Some("rho:b"),
            Some(effort("low")),
            &mut b2
        ),
        Some(effort("low"))
    );
    assert!(system_efforts(&b2).is_empty());

    // No conversation identity: never enters the map, plain top-level effort.
    let mut anon = turn(4);
    assert_eq!(
        apply(FABLE_5_1, &mut state, None, Some(effort("max")), &mut anon),
        Some(effort("max"))
    );
    assert!(system_efforts(&anon).is_empty());
    assert_eq!(state.tracked(), 2);
}

// Covers: a shared provider serving many sessions must not retain effort
// state forever; the least recently used conversation is dropped and a
// still-active one keeps its prefix
// Owner: anthropic per-message effort
#[test]
fn tracked_conversations_are_bounded_by_lru_eviction() {
    let mut state = PerMessageEffortState::default();
    let mut first = turn(1);
    apply(
        FABLE_5_1,
        &mut state,
        Some("rho:keep"),
        Some(effort("high")),
        &mut first,
    );

    for index in 0..MAX_TRACKED_CONVERSATIONS - 1 {
        let key = format!("rho:fill-{index}");
        let mut messages = turn(1);
        apply(
            FABLE_5_1,
            &mut state,
            Some(&key),
            Some(effort("low")),
            &mut messages,
        );
    }
    assert_eq!(state.tracked(), MAX_TRACKED_CONVERSATIONS);

    // Touching "keep" makes "fill-0" the oldest.
    let mut kept = turn(2);
    apply(
        FABLE_5_1,
        &mut state,
        Some("rho:keep"),
        Some(effort("high")),
        &mut kept,
    );
    let mut overflow = turn(1);
    apply(
        FABLE_5_1,
        &mut state,
        Some("rho:new"),
        Some(effort("low")),
        &mut overflow,
    );

    assert_eq!(state.tracked(), MAX_TRACKED_CONVERSATIONS);
    assert!(!state.is_tracked("rho:fill-0"));
    assert!(state.is_tracked("rho:keep"));
    assert!(state.is_tracked("rho:new"));

    // "keep" still continues with its original prefix effort.
    let mut kept_next = turn(3);
    let top = apply(
        FABLE_5_1,
        &mut state,
        Some("rho:keep"),
        Some(effort("low")),
        &mut kept_next,
    );
    assert_eq!(top, Some(effort("high")));
    assert_eq!(system_efforts(&kept_next), ["low"]);
}
