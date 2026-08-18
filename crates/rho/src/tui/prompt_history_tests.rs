use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::super::{tests::test_app, ComposerMode, HistoryDirection, InlineChoicePending};
use super::MAX_PERSISTED_PROMPT_BYTES;
use crate::prompt_history::PromptHistoryStore;

fn app_with_store() -> (TempDir, super::super::App) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prompt-history.sqlite3");
    let mut app = test_app();
    app.prompt_history.set_store_path(path);
    (directory, app)
}

fn store_for(app: &super::super::App) -> PromptHistoryStore {
    PromptHistoryStore::open_path(app.prompt_history.store_path().unwrap()).unwrap()
}

// Covers: lowering the cap below stored rows asks before deleting them.
// Owner: tui prompt-history config
#[test]
fn lowering_limit_below_stored_count_confirms() {
    let (_directory, mut app) = app_with_store();
    let store = store_for(&app);
    store.append("one", 10).unwrap();
    store.append("two", 10).unwrap();

    app.propose_prompt_history_limit(1).unwrap();
    app.settle_prompt_history();

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
    let store = store_for(&app);
    store.append("one", 10).unwrap();
    store.append("two", 10).unwrap();
    store.append("three", 10).unwrap();

    app.submit_prompt_history_limit_choice("confirm", 1)
        .unwrap();
    app.settle_prompt_history();

    let store = store_for(&app);
    assert_eq!(store.load_tail(10).unwrap(), vec!["three".to_string()]);
    assert_eq!(app.prompt_history.limit(), 1);
}

// Covers: cancelling a destructive limit change does not trim.
// Owner: tui prompt-history config
#[test]
fn cancelled_lower_limit_leaves_store() {
    let (_directory, mut app) = app_with_store();
    let store = store_for(&app);
    store.append("one", 10).unwrap();
    store.append("two", 10).unwrap();

    app.submit_prompt_history_limit_choice("cancel", 1).unwrap();
    app.settle_prompt_history();

    let store = store_for(&app);
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
    let store = store_for(&app);
    store.append("keep?", 10).unwrap();
    app.push_input_history("local");

    app.prompt_clear_prompt_history().unwrap();
    app.settle_prompt_history();
    assert!(matches!(
        app.input_ui.composer(),
        ComposerMode::InlineChoice(modal)
            if matches!(modal.pending, InlineChoicePending::ClearPromptHistory)
    ));

    app.submit_clear_prompt_history_choice("confirm").unwrap();
    app.settle_prompt_history();
    assert!(app.input_ui.history().is_empty());
    let store = store_for(&app);
    assert!(store.load_tail(10).unwrap().is_empty());
}

// Covers: oversized sent text stays in the in-memory ring but is not persisted.
// Owner: tui prompt-history persistence policy
#[test]
fn oversized_prompt_stays_in_ring_only() {
    let (_directory, mut app) = app_with_store();
    let oversized = "a".repeat(MAX_PERSISTED_PROMPT_BYTES + 1);
    app.push_input_history(&oversized);
    app.settle_prompt_history();
    assert_eq!(app.input_ui.history(), &[oversized]);
    assert!(store_for(&app).load_tail(10).unwrap().is_empty());
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

// Covers: count and clear do not create the default file just to inspect it.
// Owner: tui prompt-history persistence policy
#[test]
fn count_and_clear_do_not_create_missing_store() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prompt-history.sqlite3");
    let mut app = test_app();
    app.prompt_history.set_store_path(path.clone());

    app.prompt_clear_prompt_history().unwrap();
    app.settle_prompt_history();
    assert!(!path.exists());

    app.propose_prompt_history_limit(10).unwrap();
    app.settle_prompt_history();
    assert!(!path.exists());
}

// Covers: a disabled persist limit does not write durable appends.
// Owner: tui prompt-history persistence policy
#[test]
fn zero_limit_does_not_persist_appends() {
    let (_directory, mut app) = app_with_store();
    app.prompt_history.set_limit(0);
    app.push_input_history("hello");
    app.settle_prompt_history();
    assert_eq!(app.input_ui.history(), &["hello".to_string()]);
    assert!(store_for(&app).load_tail(10).unwrap().is_empty());
}

// Covers: enabling persistence mid-session starts writing without a restart.
// Owner: tui prompt-history persistence policy
#[test]
fn enabling_after_zero_persists_later_appends() {
    let (_directory, mut app) = app_with_store();
    app.prompt_history.set_limit(0);
    app.push_input_history("skipped");
    app.submit_prompt_history_limit_choice("confirm", 10)
        .unwrap();
    app.settle_prompt_history();
    app.push_input_history("kept");
    app.settle_prompt_history();

    assert_eq!(
        store_for(&app).load_tail(10).unwrap(),
        vec!["kept".to_string()]
    );
}

// Covers: a stale startup snapshot must not restore a confirmed clear.
// Owner: tui prompt-history load
#[test]
fn clear_before_load_does_not_reseed() {
    let (_directory, mut app) = app_with_store();
    store_for(&app).append("old", 10).unwrap();
    app.submit_clear_prompt_history_choice("confirm").unwrap();
    app.settle_prompt_history();
    assert!(app.input_ui.history().is_empty());

    assert!(!app.apply_loaded_prompt_history_seed(vec!["old".into()]));
    assert!(app.input_ui.history().is_empty());
    assert!(store_for(&app).load_tail(10).unwrap().is_empty());
}

// Covers: lowering the cap before the startup seed lands keeps the newest N.
// Owner: tui prompt-history load
#[test]
fn lower_limit_before_load_truncates_seed() {
    let mut app = test_app();
    app.prompt_history.set_limit(1);
    assert!(app.apply_loaded_prompt_history_seed(vec!["one".into(), "two".into(), "three".into()]));
    assert_eq!(app.input_ui.history(), &["three".to_string()]);
}
