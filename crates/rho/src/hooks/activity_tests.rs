use pretty_assertions::assert_eq;

use super::*;

fn activity(id: &str, outcome: HookOutcome) -> HookActivity {
    HookActivity {
        hook_id: id.into(),
        event: "before_tool_use",
        outcome,
        duration: Some(Duration::from_millis(12)),
        truncated: false,
    }
}

#[test]
fn a_denial_renders_its_hook_timing_and_reason() {
    let rendered = activity(
        "user:no-force-push",
        HookOutcome::Denied {
            reason: "denied by hook `user:no-force-push`: force push".into(),
        },
    )
    .to_string();

    assert_eq!(
        rendered,
        "user:no-force-push before_tool_use denied in 12ms: denied by hook `user:no-force-push`: force push"
    );
}

#[test]
fn a_success_renders_without_a_detail() {
    assert_eq!(
        activity("user:log", HookOutcome::Observed).to_string(),
        "user:log before_tool_use observed in 12ms"
    );
}

#[test]
fn truncated_output_is_visible_in_the_record() {
    let mut activity = activity("user:log", HookOutcome::Observed);
    activity.truncated = true;

    assert!(activity.to_string().contains("(output truncated)"));
}

#[test]
fn a_record_without_timing_omits_the_duration() {
    let mut activity = activity("user:log", HookOutcome::Dropped);
    activity.duration = None;

    assert_eq!(activity.to_string(), "user:log before_tool_use dropped");
}

#[test]
fn the_log_keeps_insertion_order() {
    let log = HookActivityLog::default();

    log.record(activity("first", HookOutcome::Continued));
    log.record(activity("second", HookOutcome::Observed));

    assert_eq!(
        log.snapshot()
            .into_iter()
            .map(|record| record.hook_id)
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

#[test]
fn the_log_drops_the_oldest_record_at_its_bound() {
    let log = HookActivityLog::default();

    for index in 0..MAX_RECORDS + 5 {
        log.record(activity(&format!("hook-{index}"), HookOutcome::Continued));
    }

    let snapshot = log.snapshot();
    assert_eq!(snapshot.len(), MAX_RECORDS);
    assert_eq!(snapshot[0].hook_id, "hook-5");
}

#[test]
fn every_outcome_has_a_stable_label() {
    assert_eq!(HookOutcome::Continued.label(), "continued");
    assert_eq!(HookOutcome::Observed.label(), "observed");
    assert_eq!(HookOutcome::Dropped.label(), "dropped");
    assert_eq!(HookOutcome::Denied { reason: "r".into() }.label(), "denied");
    assert_eq!(HookOutcome::Failed { reason: "r".into() }.label(), "failed");
}
