use pretty_assertions::assert_eq;
use serde_json::json;

use super::super::codex_continuation::CodexContinuationCandidate;
use super::{PendingSteer, SteerMatch, SteerMode};

fn pending(steer_items: Vec<serde_json::Value>) -> PendingSteer {
    PendingSteer {
        request_properties: json!({"model": "gpt-6-astra", "stream": true}),
        request_input: vec![json!({"role": "user", "content": "one"})],
        steer_items,
        mode: SteerMode::AutoContinuation,
    }
}

fn candidate(input: Vec<serde_json::Value>) -> CodexContinuationCandidate {
    CodexContinuationCandidate {
        request_properties: json!({"model": "gpt-6-astra", "stream": true}),
        input,
    }
}

// Covers: auto-continuation reuse requires the original prefix, accepted
// steers as the suffix, and no extra user/tool/config items in the middle
// Owner: openai websocket steering
#[test]
fn pending_steer_match_table() {
    let steer = vec![json!({"role": "user", "content": [{"type": "input_text", "text": "S1"}]})];
    let original = json!({"role": "user", "content": "one"});
    let assistant = json!({"role": "assistant", "content": "partial"});
    let extra_user = json!({"role": "user", "content": "extra"});
    let config = json!({"type": "configuration_update", "reasoning": {"effort": "high"}});
    let tool_out = json!({"type": "function_call_output", "call_id": "c1", "output": "ok"});

    let cases = [
        (
            "reuse with assistant middle",
            candidate(vec![original.clone(), assistant.clone(), steer[0].clone()]),
            SteerMatch::Reuse,
        ),
        (
            "extra user item",
            candidate(vec![original.clone(), extra_user, steer[0].clone()]),
            SteerMatch::FullReplay,
        ),
        (
            "configuration_update in the middle",
            candidate(vec![original.clone(), config, steer[0].clone()]),
            SteerMatch::FullReplay,
        ),
        (
            "function_call_output in the middle",
            candidate(vec![original.clone(), tool_out, steer[0].clone()]),
            SteerMatch::FullReplay,
        ),
        (
            "missing steer suffix",
            candidate(vec![original.clone(), assistant]),
            SteerMatch::FullReplay,
        ),
        (
            "rewritten prefix",
            candidate(vec![
                json!({"role":"user","content":"changed"}),
                steer[0].clone(),
            ]),
            SteerMatch::FullReplay,
        ),
    ];

    let pending = pending(steer);
    for (name, candidate, expected) in cases {
        assert_eq!(pending.matches(&candidate), expected, "{name}");
    }
}

// Covers: auto-continuation has already sampled context; reuse must tolerate
// identical projection after the steer but never silently discard an update.
// Owner: openai websocket steering
#[test]
fn pending_steer_context_match_table() {
    use crate::model::Message;
    use crate::protocol::openai_responses::codex_input_items;

    let context =
        |text: &str| codex_input_items(&[Message::model_context(text)], &mut Vec::new()).unwrap();
    let original_context = context("original");
    let steer = vec![json!({"role":"user","content":"S1"})];
    let mut active = pending(steer.clone());
    active.request_input.extend(original_context.clone());
    let history = vec![
        active.request_input[0].clone(),
        json!({"role":"assistant","content":"partial"}),
        steer[0].clone(),
    ];
    for (name, trailing, expected) in [
        ("identical", original_context.clone(), SteerMatch::Reuse),
        ("changed", context("changed"), SteerMatch::FullReplay),
        ("removed", vec![], SteerMatch::FullReplay),
        (
            "added context item",
            [original_context.clone(), context("extra")].concat(),
            SteerMatch::FullReplay,
        ),
        (
            "ordinary user suffix",
            vec![json!({"role":"user","content":"extra"})],
            SteerMatch::FullReplay,
        ),
    ] {
        assert_eq!(
            active.matches(&candidate([history.clone(), trailing].concat())),
            expected,
            "{name}"
        );
    }
    active.request_input.pop();
    assert_eq!(
        active.matches(&candidate([history, original_context].concat())),
        SteerMatch::FullReplay,
        "new context must be delivered"
    );

    // A suffix that overlaps the prefix is not an extension and must not panic.
    let pending = pending(vec![json!({"role":"user","content":"one"})]);
    assert_eq!(
        pending.matches(&candidate(pending.request_input.clone())),
        SteerMatch::FullReplay
    );
}

#[test]
fn required_input_always_replays() {
    let mut pending = pending(vec![json!({"role": "user", "content": "S1"})]);
    pending.mode = SteerMode::RequiredInput;
    let original = json!({"role": "user", "content": "one"});
    assert_eq!(
        pending.matches(&candidate(vec![
            original,
            json!({"role": "user", "content": "S1"})
        ])),
        SteerMatch::FullReplay
    );
}
