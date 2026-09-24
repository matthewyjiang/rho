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
