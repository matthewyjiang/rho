use super::*;

fn call_id(value: &str) -> ToolCallId {
    ToolCallId::from_string(value).unwrap()
}

fn live_labels(batch: &ToolCallBatch) -> Vec<&str> {
    batch
        .live_entries()
        .map(|entry| entry.display_lines[0].as_str())
        .collect()
}

#[test]
fn promotion_preserves_model_order_instead_of_call_id_order() {
    let mut batch = ToolCallBatch::default();
    let first = call_id("z-model-first");
    let second = call_id("a-model-second");
    batch.preview(0, Some(first.clone()), vec!["first preview".into()], None);
    batch.preview(1, Some(second.clone()), vec!["second preview".into()], None);

    batch.started(second, vec!["second running".into()], None);
    assert_eq!(live_labels(&batch), ["first preview", "second running"]);

    batch.started(first, vec!["first running".into()], None);
    assert_eq!(live_labels(&batch), ["first running", "second running"]);
}

#[test]
fn proposal_reuses_stream_preview_slot_when_call_id_matches() {
    let mut batch = ToolCallBatch::default();
    let call = call_id("call-agent");
    // Responses output_index can land past non-tool items (reasoning, etc.).
    batch.preview(
        3,
        Some(call.clone()),
        vec!["● reviewer  starting".into(), "  …Return: verdict".into()],
        None,
    );
    // Proposal is call-id keyed and must not mint a second card.
    batch.preview_call(
        call.clone(),
        vec![
            "● reviewer  starting".into(),
            "  Perform a thermo-nuclear review".into(),
        ],
        None,
    );
    batch.started(
        call,
        vec![
            "● reviewer  running".into(),
            "  Perform a thermo-nuclear review".into(),
            "  ab12cd · rho attach ab12cd".into(),
        ],
        None,
    );

    assert_eq!(batch.live_entries().count(), 1);
    assert_eq!(live_labels(&batch), ["● reviewer  running"]);
    assert!(batch.previews.is_empty());
}

#[test]
fn proposal_without_stream_appends_in_arrival_order() {
    let mut batch = ToolCallBatch::default();
    let first = call_id("call-a");
    let second = call_id("call-b");
    batch.preview_call(first.clone(), vec!["first starting".into()], None);
    batch.preview_call(second.clone(), vec!["second starting".into()], None);
    batch.started(first, vec!["first running".into()], None);
    batch.started(second, vec!["second running".into()], None);

    assert_eq!(live_labels(&batch), ["first running", "second running"]);
}

#[test]
fn late_stream_preview_is_ignored_after_start() {
    let mut batch = ToolCallBatch::default();
    let call = call_id("call-agent");
    batch.preview(3, Some(call.clone()), vec!["starting".into()], None);
    batch.started(call.clone(), vec!["running".into()], None);
    batch.preview(3, Some(call), vec!["stale starting".into()], None);
    batch.preview(3, None, vec!["index only stale".into()], None);

    assert_eq!(live_labels(&batch), ["running"]);
    assert!(batch.previews.is_empty());
}

#[test]
fn latest_is_last_model_order_entry_when_later_entry_is_still_a_preview() {
    let mut batch = ToolCallBatch::default();
    let first = call_id("z-model-first");
    let second = call_id("a-model-second");
    batch.preview(0, Some(first.clone()), vec!["first".into()], None);
    batch.preview(1, Some(second), vec!["second".into()], None);
    batch.started(first.clone(), vec!["first running".into()], None);

    batch.latest_mut().unwrap().expanded = true;

    assert!(!batch.running[&first].expanded);
    assert!(batch.previews[&1].expanded);
}

#[test]
fn latest_is_last_model_order_entry_after_promotion() {
    let mut batch = ToolCallBatch::default();
    let first = call_id("z-model-first");
    let second = call_id("a-model-second");
    batch.preview(0, Some(first.clone()), vec!["first".into()], None);
    batch.preview(1, Some(second.clone()), vec!["second".into()], None);
    batch.started(first, vec!["first running".into()], None);
    batch.started(second.clone(), vec!["second running".into()], None);

    batch.latest_mut().unwrap().expanded = true;

    assert!(!batch.running[&call_id("z-model-first")].expanded);
    assert!(batch.running[&second].expanded);
}
