use std::{
    io,
    sync::{Arc, Mutex},
    time::Instant,
};

use crossterm::event::{MouseButton, MouseEventKind};
use pretty_assertions::assert_eq;
use ratatui::layout::Rect;

use super::*;
use crate::tui::clipboard::{Clipboard, CopyOutcome};

struct FakeClipboard {
    text: Arc<Mutex<String>>,
}

impl Clipboard for FakeClipboard {
    fn copy(&mut self, text: &str) -> io::Result<CopyOutcome> {
        *self.text.lock().expect("clipboard lock") = text.to_string();
        Ok(CopyOutcome::Confirmed)
    }

    fn paste(&mut self) -> io::Result<String> {
        Ok(self.text.lock().expect("clipboard lock").clone())
    }
}

// Covers: /info must paint the local snapshot before external probes return.
// Owner: interactive TUI (unit seam; PTY covers the visible overlay)
#[test]
fn opening_info_paints_local_fields_before_probes() {
    let mut app = super::super::tests::test_app();
    app.execute_info_command().unwrap();

    assert!(matches!(
        app.input_ui.composer(),
        ComposerMode::Panel(PanelOverlay::Info(_))
    ));
    assert!(app.pending_info_runtimes.is_none());
    assert!(app.pending_info_tree.is_none());
    let ComposerMode::Panel(PanelOverlay::Info(overlay)) = app.input_ui.composer() else {
        unreachable!("overlay just opened");
    };
    assert_eq!(
        overlay.info.external_runtimes(),
        crate::tui::info_command::checking_external_runtimes()
    );
    assert!(
        app.history.entries().is_empty(),
        "info overlay must not insert a transcript row"
    );
}

// Covers: a finished probe replaces the checking rows on the open overlay.
// Owner: interactive TUI (unit seam)
#[tokio::test]
async fn finished_runtime_probe_fills_the_open_overlay() {
    let mut app = super::super::tests::test_app();
    app.execute_info_command().unwrap();
    app.pending_info_runtimes = Some(tokio::spawn(async {
        vec![
            "claude code: signed in as test".into(),
            "cursor: signed in".into(),
        ]
    }));
    while app
        .pending_info_runtimes
        .as_ref()
        .is_some_and(|handle| !handle.is_finished())
    {
        tokio::task::yield_now().await;
    }

    assert!(app.poll_info_refresh().await.unwrap());
    assert!(app.pending_info_runtimes.is_none());
    let ComposerMode::Panel(PanelOverlay::Info(overlay)) = app.input_ui.composer() else {
        panic!("probe closed the overlay");
    };
    assert_eq!(
        overlay.info.external_runtimes(),
        &[
            "claude code: signed in as test".to_string(),
            "cursor: signed in".to_string(),
        ]
    );
}

// Covers: /info opened mid-turn keeps the unavailable note, then loads the
// tree once the session is idle again.
// Owner: interactive TUI (unit seam)
#[test]
fn info_opened_during_a_turn_defers_the_tree_read_until_idle() {
    let mut app = super::super::tests::test_app();
    app.info.session.session_id = Some("session-1".into());
    app.begin_provider_turn_ui();
    app.execute_info_command().unwrap();

    assert!(app.info_tree_deferred);
    assert!(app.pending_info_tree.is_none());
    let ComposerMode::Panel(PanelOverlay::Info(overlay)) = app.input_ui.composer() else {
        panic!("overlay did not open");
    };
    assert!(!overlay.info.tree_loading());

    assert!(!app.start_deferred_info_tree());
    app.end_busy_ui();
    assert!(app.start_deferred_info_tree());
    assert!(!app.info_tree_deferred);
    let ComposerMode::Panel(PanelOverlay::Info(overlay)) = app.input_ui.composer() else {
        panic!("overlay closed before the deferred read");
    };
    assert!(overlay.info.tree_loading());
}

// Covers: closing must stop the probe task, not just forget the handle.
// Owner: interactive TUI (unit seam)
#[tokio::test]
async fn cancelling_info_probes_waits_for_the_task_to_stop() {
    let mut app = super::super::tests::test_app();
    let task_marker = Arc::new(());
    let captured_marker = task_marker.clone();
    app.pending_info_runtimes = Some(tokio::spawn(async move {
        let _marker = captured_marker;
        std::future::pending::<Vec<String>>().await
    }));

    app.cancel_info_refresh().await;

    assert!(app.pending_info_runtimes.is_none());
    assert_eq!(Arc::strong_count(&task_marker), 1);
}

// Covers: closing drops probe handles so a later poll cannot apply them.
// Owner: interactive TUI (unit seam)
#[tokio::test]
async fn closing_info_clears_probe_handles() {
    let mut app = super::super::tests::test_app();
    app.execute_info_command().unwrap();
    app.pending_info_runtimes = Some(tokio::spawn(std::future::pending()));
    app.close_info_overlay();

    assert!(app.pending_info_runtimes.is_none());
    assert!(!matches!(
        app.input_ui.composer(),
        ComposerMode::Panel(PanelOverlay::Info(_))
    ));
    tokio::task::yield_now().await;
}

// Covers: c copies the report text and leaves the overlay open.
// Owner: interactive TUI (unit seam; PTY covers the key)
#[test]
fn copy_key_writes_the_report_without_closing() {
    let mut app = super::super::tests::test_app();
    let copied = Arc::new(Mutex::new(String::new()));
    app.clipboard = Box::new(FakeClipboard {
        text: Arc::clone(&copied),
    });
    app.execute_info_command().unwrap();

    app.copy_info_report(Instant::now());

    assert!(matches!(
        app.input_ui.composer(),
        ComposerMode::Panel(PanelOverlay::Info(_))
    ));
    let text = copied.lock().expect("clipboard lock").clone();
    assert!(
        text.contains("openai"),
        "copied report missing provider:\n{text}"
    );
    assert!(
        !text.contains('┌'),
        "copied report included overlay chrome:\n{text}"
    );
}

// Covers: a drag inside the overlay copies that span, not the whole report.
// Owner: interactive TUI (unit seam; PTY covers the highlight)
#[test]
fn drag_copies_the_selected_span() {
    let mut app = super::super::tests::test_app();
    let copied = Arc::new(Mutex::new(String::new()));
    app.clipboard = Box::new(FakeClipboard {
        text: Arc::clone(&copied),
    });
    app.execute_info_command().unwrap();
    let screen = Rect::new(0, 0, 100, 40);
    let frame = app.info_overlay_frame(screen).expect("info overlay");
    let body = frame.body();
    let row = body.y;
    let start = body.x;

    app.handle_panel_overlay_mouse(
        MouseEventKind::Down(MouseButton::Left),
        screen,
        start,
        row,
        Instant::now(),
    );
    app.handle_panel_overlay_mouse(
        MouseEventKind::Drag(MouseButton::Left),
        screen,
        start.saturating_add(3),
        row,
        Instant::now(),
    );
    app.handle_panel_overlay_mouse(
        MouseEventKind::Up(MouseButton::Left),
        screen,
        start.saturating_add(3),
        row,
        Instant::now(),
    );

    let text = copied.lock().expect("clipboard lock").clone();
    assert!(!text.is_empty(), "drag did not copy");
    assert!(
        !text.contains("Workspace"),
        "drag copied the whole report:\n{text}"
    );
    assert!(matches!(
        app.input_ui.composer(),
        ComposerMode::Panel(PanelOverlay::Info(_))
    ));
}
