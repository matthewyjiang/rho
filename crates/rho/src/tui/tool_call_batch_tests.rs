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
    batch.preview(0, Some(first.clone()), vec!["first preview".into()]);
    batch.preview(1, Some(second.clone()), vec!["second preview".into()]);

    batch.started(second, vec!["second running".into()]);
    assert_eq!(live_labels(&batch), ["first preview", "second running"]);

    batch.started(first, vec!["first running".into()]);
    assert_eq!(live_labels(&batch), ["first running", "second running"]);
}

#[test]
fn latest_is_last_model_order_entry_when_later_entry_is_still_a_preview() {
    let mut batch = ToolCallBatch::default();
    let first = call_id("z-model-first");
    let second = call_id("a-model-second");
    batch.preview(0, Some(first.clone()), vec!["first".into()]);
    batch.preview(1, Some(second), vec!["second".into()]);
    batch.started(first.clone(), vec!["first running".into()]);

    batch.latest_mut().unwrap().expanded = true;

    assert!(!batch.running[&first].expanded);
    assert!(batch.previews[&1].expanded);
}

#[test]
fn latest_is_last_model_order_entry_after_promotion() {
    let mut batch = ToolCallBatch::default();
    let first = call_id("z-model-first");
    let second = call_id("a-model-second");
    batch.preview(0, Some(first.clone()), vec!["first".into()]);
    batch.preview(1, Some(second.clone()), vec!["second".into()]);
    batch.started(first, vec!["first running".into()]);
    batch.started(second.clone(), vec!["second running".into()]);

    batch.latest_mut().unwrap().expanded = true;

    assert!(!batch.running[&call_id("z-model-first")].expanded);
    assert!(batch.running[&second].expanded);
}
