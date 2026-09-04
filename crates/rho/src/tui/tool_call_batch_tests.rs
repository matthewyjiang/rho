use rho_tools::tool_card::{ToolCard, ToolFamily, ToolHeader, ToolStatus};

use super::*;

fn call_id(value: &str) -> ToolCallId {
    ToolCallId::from_string(value).unwrap()
}

fn card(label: &str) -> ToolCard {
    ToolCard::new(
        ToolStatus::Running,
        ToolFamily::Default,
        ToolHeader::call(label, None),
    )
}

fn live_labels(batch: &ToolCallBatch) -> Vec<String> {
    batch
        .live_entries()
        .map(|entry| entry.card.header_text())
        .collect()
}

#[test]
fn promotion_preserves_model_order_instead_of_call_id_order() {
    let mut batch = ToolCallBatch::default();
    let first = call_id("z-model-first");
    let second = call_id("a-model-second");
    batch.preview(0, Some(first.clone()), Some(card("first preview")));
    batch.preview(1, Some(second.clone()), Some(card("second preview")));

    batch.started(second, card("second running"));
    assert_eq!(live_labels(&batch), ["● first preview", "● second running"]);

    batch.started(first, card("first running"));
    assert_eq!(live_labels(&batch), ["● first running", "● second running"]);
}

#[test]
fn proposal_reuses_stream_preview_slot_when_call_id_matches() {
    let mut batch = ToolCallBatch::default();
    let call = call_id("call-agent");
    // Responses output_index can land past non-tool items (reasoning, etc.).
    batch.preview(3, Some(call.clone()), Some(card("reviewer starting")));
    // Proposal is call-id keyed and must not mint a second card.
    batch.preview_call(call.clone(), card("reviewer proposed"));
    batch.started(call, card("reviewer running"));

    assert_eq!(batch.live_entries().count(), 1);
    assert_eq!(live_labels(&batch), ["● reviewer running"]);
    assert!(batch.previews.is_empty());
}

#[test]
fn proposal_without_stream_appends_in_arrival_order() {
    let mut batch = ToolCallBatch::default();
    let first = call_id("call-a");
    let second = call_id("call-b");
    batch.preview_call(first.clone(), card("first starting"));
    batch.preview_call(second.clone(), card("second starting"));
    batch.started(first, card("first running"));
    batch.started(second, card("second running"));

    assert_eq!(live_labels(&batch), ["● first running", "● second running"]);
}

#[test]
fn late_stream_preview_is_ignored_after_start() {
    let mut batch = ToolCallBatch::default();
    let call = call_id("call-agent");
    batch.preview(3, Some(call.clone()), Some(card("starting")));
    batch.started(call.clone(), card("running"));
    batch.preview(3, Some(call), Some(card("stale starting")));
    batch.preview(3, None, Some(card("index only stale")));

    assert_eq!(live_labels(&batch), ["● running"]);
    assert!(batch.previews.is_empty());
}

#[test]
fn latest_is_last_model_order_entry_when_later_entry_is_still_a_preview() {
    let mut batch = ToolCallBatch::default();
    let first = call_id("z-model-first");
    let second = call_id("a-model-second");
    batch.preview(0, Some(first.clone()), Some(card("first")));
    batch.preview(1, Some(second), Some(card("second")));
    batch.started(first.clone(), card("first running"));

    batch.latest_mut().unwrap().expanded = true;

    assert!(!batch.running[&first].expanded);
    assert!(batch.previews[&1].expanded);
}

#[test]
fn latest_is_last_model_order_entry_after_promotion() {
    let mut batch = ToolCallBatch::default();
    let first = call_id("z-model-first");
    let second = call_id("a-model-second");
    batch.preview(0, Some(first.clone()), Some(card("first")));
    batch.preview(1, Some(second.clone()), Some(card("second")));
    batch.started(first, card("first running"));
    batch.started(second.clone(), card("second running"));

    batch.latest_mut().unwrap().expanded = true;

    assert!(!batch.running[&call_id("z-model-first")].expanded);
    assert!(batch.running[&second].expanded);
}

#[test]
fn finished_removes_running_and_returns_expanded() {
    let mut batch = ToolCallBatch::default();
    let call = call_id("call-1");
    batch.started(call.clone(), card("running"));
    batch.latest_mut().unwrap().expanded = true;
    assert!(batch.finished(&call));
    assert!(!batch.is_running());
    assert!(batch.live_entries().next().is_none());
}

#[test]
fn unbound_index_preview_is_a_separate_slot_from_call_id_proposal() {
    // Batch binding is call-id keyed. An index-only preview cannot be claimed by
    // a later proposal for a different address space.
    let mut batch = ToolCallBatch::default();
    let call = call_id("call-dup");
    batch.preview(0, None, Some(card("streamed without id")));
    batch.preview_call(call.clone(), card("proposed"));

    assert_eq!(batch.live_entries().count(), 2);
    assert_eq!(live_labels(&batch), ["● streamed without id", "● proposed"]);

    batch.started(call, card("running"));
    assert_eq!(batch.live_entries().count(), 2);
    assert_eq!(live_labels(&batch), ["● streamed without id", "● running"]);
}

#[test]
fn identity_only_preview_binds_call_id_without_replacing_card() {
    let mut batch = ToolCallBatch::default();
    let call = call_id("call-late-id");
    batch.preview(0, None, Some(card("streamed")));
    batch.preview(0, Some(call.clone()), None);

    assert_eq!(live_labels(&batch), ["● streamed"]);
    batch.preview_call(call.clone(), card("proposed"));
    batch.started(call, card("running"));
    assert_eq!(live_labels(&batch), ["● running"]);
    assert!(batch.previews.is_empty());
}

// Covers: shell elapsed clock must not reset when progress replaces the card
// Owner: pure unit (tool call batch)
#[test]
fn updates_preserve_started_at_from_the_first_running_card() {
    let mut batch = ToolCallBatch::default();
    let call = call_id("call-shell");
    batch.started(call.clone(), card("running"));
    let started_at = batch.running[&call]
        .started_at
        .expect("timer starts on start");
    batch.updated(call.clone(), card("still running"));
    assert_eq!(batch.running[&call].started_at, Some(started_at));
}

// Covers: argument-stream previews must not start the shell elapsed clock
// Owner: pure unit (tool call batch)
#[test]
fn previews_do_not_start_elapsed_until_started() {
    let mut batch = ToolCallBatch::default();
    let call = call_id("call-shell");
    batch.preview_call(call.clone(), card("preview"));
    assert!(batch
        .previews
        .values()
        .all(|entry| entry.started_at.is_none()));
    batch.started(call.clone(), card("running"));
    assert!(batch.running[&call].started_at.is_some());
}

// Covers: interrupted rows must not carry a live clock into the retained feed
// Owner: pure unit (tool call batch)
#[test]
fn interrupted_entries_drop_the_live_clock() {
    let mut batch = ToolCallBatch::default();
    let call = call_id("call-shell");
    batch.started(call.clone(), card("running"));
    let interrupted = batch.interrupted_entries();
    assert!(interrupted.iter().all(|entry| entry.started_at.is_none()));
    assert!(interrupted
        .iter()
        .all(|entry| entry.card.status == ToolStatus::Interrupted));
}

// Covers: detached cards survive clear and leave only when finished.
// Owner: pure unit (tool call batch)
#[test]
fn detached_survives_clear_and_finished_evicts() {
    struct Case {
        name: &'static str,
        finish: bool,
        expected_live: usize,
    }
    let cases = [
        Case {
            name: "detached survives clear",
            finish: false,
            expected_live: 1,
        },
        Case {
            name: "finished evicts detached",
            finish: true,
            expected_live: 0,
        },
    ];
    for case in cases {
        let mut batch = ToolCallBatch::default();
        let call = call_id("call-agent");
        batch.started(call.clone(), card("running"));
        batch.detach(call.clone());
        batch.clear();
        if case.finish {
            batch.finished(&call);
        }
        assert_eq!(
            batch.live_entries().count(),
            case.expected_live,
            "{}",
            case.name
        );
        assert_eq!(batch.is_running(), case.expected_live > 0, "{}", case.name);
    }
}
