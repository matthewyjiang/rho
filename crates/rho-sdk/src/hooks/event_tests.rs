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
