use pretty_assertions::assert_eq;

use super::*;

impl App {
    fn spend_overlay(&self) -> Option<&SpendOverlay> {
        match self.input_ui.composer() {
            ComposerMode::Panel(PanelOverlay::Spend(overlay)) => Some(overlay),
            _ => None,
        }
    }
}

fn reports() -> Arc<SpendReports> {
    let now = chrono::NaiveDate::from_ymd_opt(2026, 3, 10)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap();
    Arc::new(SpendReports::empty(now))
}

/// Whether the open overlay shows `expected` reports, and its load status.
fn shown(app: &App, expected: &Arc<SpendReports>) -> (bool, LoadStatus) {
    let overlay = app.spend_overlay().expect("spend overlay open");
    let same = overlay
        .reports
        .as_ref()
        .is_some_and(|reports| Arc::ptr_eq(reports, expected));
    (same, overlay.load.clone())
}

fn start_load(app: &mut App) -> tokio::sync::oneshot::Sender<LoadResult> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.tasks.spawn(
        TaskId::SpendLoad,
        async move { receiver.await.unwrap() },
        spend_load_output,
    );
    sender
}

async fn finish_load(
    app: &mut App,
    sender: tokio::sync::oneshot::Sender<LoadResult>,
    result: LoadResult,
) {
    sender.send(result).unwrap();
    while !app.tasks.has_finished() {
        tokio::task::yield_now().await;
    }
    app.apply_finished_ui_tasks();
}

// Covers: reopening /spend paints the last reports at once while a read runs,
// a read in flight is reused rather than stacked, a read that lands while the
// overlay is closed still fills the cache, and a failed refresh keeps the
// reports while saying they are stale.
// Owner: interactive TUI (unit seam; PTY covers the visible overlay)
#[tokio::test]
async fn cache_serves_reopens_and_survives_failed_refresh() {
    let mut app = super::super::tests::test_app();
    let first = reports();

    // First open: nothing cached, so the spinner shows.
    let sender = start_load(&mut app);
    app.execute_spend_command().unwrap();
    assert!(app.spend_overlay().unwrap().shows_spinner());

    // Closing and reopening mid-read reuses the same read.
    app.close_panel_overlay();
    app.execute_spend_command().unwrap();
    assert!(app.spend_loading());

    // The read lands after close; the cache still keeps it.
    app.close_panel_overlay();
    finish_load(&mut app, sender, Ok(Arc::clone(&first))).await;
    assert!(app
        .spend
        .reports
        .as_ref()
        .is_some_and(|r| Arc::ptr_eq(r, &first)));

    // Reopen paints the cached reports while a fresh read runs.
    let sender = start_load(&mut app);
    app.execute_spend_command().unwrap();
    let (same, load) = shown(&app, &first);
    assert!(
        same && matches!(load, LoadStatus::Running { .. }),
        "{load:?}"
    );

    finish_load(&mut app, sender, Err("database is locked".into())).await;
    assert_eq!(
        shown(&app, &first),
        (true, LoadStatus::Failed("database is locked".into()))
    );
}
