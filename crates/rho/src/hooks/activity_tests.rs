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

// Covers: the log keeps insertion order and drops the oldest records once it
// reaches its bound.
// Owner: hook activity log
#[test]
fn the_log_keeps_order_and_drops_the_oldest_record_at_its_bound() {
    let log = HookActivityLog::default();

    for index in 0..MAX_RECORDS + 5 {
        log.record(activity(&format!("hook-{index}"), HookOutcome::Continued));
    }

    assert_eq!(
        log.snapshot()
            .into_iter()
            .map(|record| record.hook_id)
            .collect::<Vec<_>>(),
        (5..MAX_RECORDS + 5)
            .map(|index| format!("hook-{index}"))
            .collect::<Vec<_>>()
    );
}
