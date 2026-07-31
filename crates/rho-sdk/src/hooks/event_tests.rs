use pretty_assertions::assert_eq;

use super::*;

#[test]
fn wire_names_round_trip_for_every_named_event() {
    for event in HookEventKind::ALL {
        assert_eq!(
            HookEventKind::from_wire_name(event.wire_name()),
            Some(*event),
            "{event} did not round-trip through its wire name"
        );
    }
}

#[test]
fn wire_names_are_unique() {
    let mut names: Vec<_> = HookEventKind::ALL
        .iter()
        .map(|event| event.wire_name())
        .collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total);
}

#[test]
fn unknown_event_names_do_not_resolve() {
    assert_eq!(HookEventKind::from_wire_name("before_tool"), None);
    assert_eq!(HookEventKind::from_wire_name(""), None);
    assert_eq!(HookEventKind::from_wire_name("BeforeToolUse"), None);
    assert_eq!(HookEventKind::from_wire_name("user_prompt_accepted"), None);
    assert_eq!(HookEventKind::from_wire_name("turn_completed"), None);
}

#[test]
fn only_before_tool_use_blocks() {
    let blocking: Vec<_> = HookEventKind::ALL
        .iter()
        .copied()
        .filter(|event| event.is_blocking())
        .collect();
    assert_eq!(blocking, vec![HookEventKind::BeforeToolUse]);
}

#[test]
fn every_event_has_an_explicit_post_action_classification() {
    let classifications = HookEventKind::ALL
        .iter()
        .map(|event| (*event, event.is_post_action()))
        .collect::<Vec<_>>();
    assert_eq!(
        classifications,
        vec![
            (HookEventKind::SessionStarted, false),
            (HookEventKind::BeforeToolUse, false),
            (HookEventKind::AfterToolUse, true),
            (HookEventKind::RunCompleted, true),
            (HookEventKind::RunFailed, true),
            (HookEventKind::SessionCompleted, false),
            (HookEventKind::SessionFailed, true),
        ]
    );
}

#[test]
fn version_one_delivers_exactly_the_documented_events() {
    let delivered: Vec<_> = HookEventKind::ALL
        .iter()
        .copied()
        .map(HookEventKind::wire_name)
        .collect();
    assert_eq!(
        delivered,
        vec![
            "session_started",
            "before_tool_use",
            "after_tool_use",
            "run_completed",
            "run_failed",
            "session_completed",
            "session_failed",
        ]
    );
}

#[test]
fn serialized_event_matches_its_wire_name() {
    for event in HookEventKind::ALL {
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::Value::String(event.wire_name().to_owned())
        );
    }
}
