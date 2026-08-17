use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::super::{
    prompt_history_persistence::MAX_PERSISTED_PROMPT_BYTES, tests::test_app, ComposerMode,
    HistoryDirection, InlineChoicePending,
};
use crate::prompt_history::PromptHistoryStore;

fn app_with_store() -> (TempDir, super::super::App) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prompt-history.sqlite3");
    let mut app = test_app();
    app.prompt_history_store_path = Some(path);
    (directory, app)
}

// Covers: lowering the cap below stored rows asks before deleting them.
// Owner: tui prompt-history config
#[test]
fn lowering_limit_below_stored_count_confirms() {
    let (_directory, mut app) = app_with_store();
    let store =
        PromptHistoryStore::open_path(app.prompt_history_store_path.as_ref().unwrap()).unwrap();
    store.append("one", 1, 10).unwrap();
    store.append("two", 2, 10).unwrap();

    app.propose_prompt_history_limit(1).unwrap();

    assert!(matches!(
        app.input_ui.composer(),
        ComposerMode::InlineChoice(modal)
            if matches!(modal.pending, InlineChoicePending::PromptHistoryLimit { new_limit: 1 })
    ));
}

// Covers: confirming a lower cap trims the store immediately.
// Owner: tui prompt-history config
#[test]
fn confirmed_lower_limit_trims_store() {
    let (_directory, mut app) = app_with_store();
    let path = app.prompt_history_store_path.clone().unwrap();
    let store = PromptHistoryStore::open_path(&path).unwrap();
    store.append("one", 1, 10).unwrap();
    store.append("two", 2, 10).unwrap();
    store.append("three", 3, 10).unwrap();

    app.submit_prompt_history_limit_choice("confirm", 1)
        .unwrap();

    let store = PromptHistoryStore::open_path(&path).unwrap();
    assert_eq!(store.load_tail(10).unwrap(), vec!["three".to_string()]);
    assert_eq!(app.prompt_history_limit, 1);
}

// Covers: cancelling a destructive limit change does not trim.
// Owner: tui prompt-history config
#[test]
fn cancelled_lower_limit_leaves_store() {
    let (_directory, mut app) = app_with_store();
    let path = app.prompt_history_store_path.clone().unwrap();
    let store = PromptHistoryStore::open_path(&path).unwrap();
    store.append("one", 1, 10).unwrap();
    store.append("two", 2, 10).unwrap();

    app.submit_prompt_history_limit_choice("cancel", 1).unwrap();

    let store = PromptHistoryStore::open_path(&path).unwrap();
    assert_eq!(
        store.load_tail(10).unwrap(),
        vec!["one".to_string(), "two".to_string()]
    );
}

// Covers: clear asks first, then wipes ring and store.
// Owner: tui prompt-history config
#[test]
fn clear_confirms_then_wipes() {
    let (_directory, mut app) = app_with_store();
    let path = app.prompt_history_store_path.clone().unwrap();
    let store = PromptHistoryStore::open_path(&path).unwrap();
    store.append("keep?", 1, 10).unwrap();
    app.push_input_history("local");

    app.prompt_clear_prompt_history().unwrap();
    assert!(matches!(
        app.input_ui.composer(),
        ComposerMode::InlineChoice(modal)
            if matches!(modal.pending, InlineChoicePending::ClearPromptHistory)
    ));

    app.submit_clear_prompt_history_choice("confirm").unwrap();
    assert!(app.input_ui.history().is_empty());
    let store = PromptHistoryStore::open_path(&path).unwrap();
    assert!(store.load_tail(10).unwrap().is_empty());
}

// Covers: oversized sent text stays in the in-memory ring but is not persisted.
// Owner: tui prompt-history persistence policy
#[test]
fn oversized_prompt_stays_in_ring_only() {
    let mut app = test_app();
    let oversized = "a".repeat(MAX_PERSISTED_PROMPT_BYTES + 1);
    app.push_input_history(&oversized);
    assert_eq!(app.input_ui.history(), &[oversized]);
    let mut rx = app.take_prompt_history_rx().unwrap();
    assert!(rx.try_recv().is_err());
}

// Covers: seeding older entries in front of local history preserves recall
// cursor so an in-progress up-arrow does not jump.
// Owner: tui input history
#[test]
fn seed_history_front_offsets_in_progress_recall() {
    let mut app = test_app();
    app.push_input_history("local");
    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input_ui.text(), "local");
    assert_eq!(app.input_ui.history_cursor(), Some(0));

    app.input_ui
        .seed_history_front(vec!["older".into(), "local".into()]);
    assert_eq!(
        app.input_ui.history(),
        &["older".to_string(), "local".to_string()]
    );
    assert_eq!(app.input_ui.history_cursor(), Some(1));
    assert_eq!(app.input_ui.text(), "local");

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input_ui.text(), "older");
}

// Covers: a disabled persist limit does not enqueue durable appends.
// Owner: tui prompt-history persistence policy
#[test]
fn zero_limit_does_not_enqueue_appends() {
    let mut app = test_app();
    app.prompt_history_limit = 0;
    app.push_input_history("hello");
    assert_eq!(app.input_ui.history(), &["hello".to_string()]);
    let mut rx = app.take_prompt_history_rx().unwrap();
    assert!(rx.try_recv().is_err());
}
