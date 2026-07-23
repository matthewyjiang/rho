use crate::tui::clipboard::CopyOutcome;

use super::*;

#[derive(Clone)]
struct RecordingClipboard {
    copied: Arc<Mutex<Vec<String>>>,
}

impl ClipboardWriter for RecordingClipboard {
    fn copy(&mut self, text: &str) -> std::io::Result<CopyOutcome> {
        self.copied.lock().unwrap().push(text.to_string());
        Ok(CopyOutcome::Confirmed)
    }
}

struct OutcomeClipboard(CopyOutcome);

impl ClipboardWriter for OutcomeClipboard {
    fn copy(&mut self, _text: &str) -> std::io::Result<CopyOutcome> {
        Ok(self.0)
    }
}

struct FailingClipboard;

impl ClipboardWriter for FailingClipboard {
    fn copy(&mut self, _text: &str) -> std::io::Result<CopyOutcome> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "terminal closed",
        ))
    }
}

#[test]
fn terminal_clipboard_request_is_not_reported_as_a_confirmed_copy() {
    let mut app = test_app();
    app.clipboard = Box::new(OutcomeClipboard(CopyOutcome::SentToTerminal));

    app.copy_text("hello", Instant::now());

    assert_eq!(
        app.history.copy_notice().unwrap().message(),
        "5 chars sent to terminal"
    );
}

#[test]
fn clipboard_write_failure_is_reported() {
    let mut app = test_app();
    app.clipboard = Box::new(FailingClipboard);

    app.copy_text("hello", Instant::now());

    assert_eq!(
        app.history.copy_notice().unwrap().message(),
        "copy failed: terminal closed"
    );
}

#[test]
fn clicking_expandable_tool_output_toggles_the_clicked_entry() {
    let mut app = test_app();
    app.info.runtime.max_tool_output_lines = 1;
    app.record_inserted_entry(test_tool_entry(
        true,
        &["write_file", "first\nsecond\nthird"],
    ));
    app.record_inserted_entry(test_tool_entry(true, &["bash", "alpha\nbeta\ngamma"]));
    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();

    let now = Instant::now();
    let history_len = app.history_len(60, now);
    let layout = app.screen_layout(Rect::new(0, 0, 60, 24), now);
    let history_start = app.visible_history_start(history_len, layout.history.height as usize);
    let prompt_line = app
        .history_lines(60, now)
        .iter()
        .position(|line| line_text(line).contains("ctrl+o to expand"))
        .unwrap();
    let row = layout.history.y + prompt_line.saturating_sub(history_start) as u16;
    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        2,
        row,
        &mut terminal,
    )
    .unwrap();
    app.handle_mouse_event(MouseEventKind::Up(MouseButton::Left), 2, row, &mut terminal)
        .unwrap();

    assert!(matches!(
        app.history.entries().first(),
        Some(Entry::Tool(ToolEntry { expanded: true, .. }))
    ));
    assert!(matches!(
        app.history.last(),
        Some(Entry::Tool(ToolEntry {
            expanded: false,
            ..
        }))
    ));
    assert_eq!(app.status, "tool output expanded");

    let now = Instant::now();
    let history_len = app.history_len(60, now);
    let layout = app.screen_layout(Rect::new(0, 0, 60, 24), now);
    let history_start = app.visible_history_start(history_len, layout.history.height as usize);
    let prompt_line = app
        .history_lines(60, now)
        .iter()
        .position(|line| line_text(line).contains("ctrl+o to expand"))
        .unwrap();
    let row = layout.history.y + prompt_line.saturating_sub(history_start) as u16;
    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        2,
        row,
        &mut terminal,
    )
    .unwrap();
    app.handle_mouse_event(MouseEventKind::Up(MouseButton::Left), 2, row, &mut terminal)
        .unwrap();

    assert!(matches!(
        app.history.entries().first(),
        Some(Entry::Tool(ToolEntry {
            expanded: false,
            ..
        }))
    ));
    assert!(matches!(
        app.history.last(),
        Some(Entry::Tool(ToolEntry { expanded: true, .. }))
    ));
    assert_eq!(
        app.history_lines(60, Instant::now())
            .iter()
            .filter(|line| line_text(line).contains("ctrl+o to collapse"))
            .count(),
        1
    );

    let now = Instant::now();
    let history_len = app.history_len(60, now);
    let layout = app.screen_layout(Rect::new(0, 0, 60, 24), now);
    let history_start = app.visible_history_start(history_len, layout.history.height as usize);
    let prompt_line = app
        .history_lines(60, now)
        .iter()
        .position(|line| line_text(line).contains("ctrl+o to collapse"))
        .unwrap();
    let row = layout.history.y + prompt_line.saturating_sub(history_start) as u16;
    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        2,
        row,
        &mut terminal,
    )
    .unwrap();
    app.handle_mouse_event(MouseEventKind::Up(MouseButton::Left), 2, row, &mut terminal)
        .unwrap();

    assert!(matches!(
        app.history.entries().first(),
        Some(Entry::Tool(ToolEntry {
            expanded: false,
            ..
        }))
    ));
    assert!(matches!(
        app.history.last(),
        Some(Entry::Tool(ToolEntry {
            expanded: false,
            ..
        }))
    ));
    assert_eq!(app.status, "tool output collapsed");
}

#[test]
fn clicking_expandable_pending_tool_output_toggles_it() {
    let mut app = test_app();
    app.info.runtime.max_tool_output_lines = 1;
    app.tool_calls
        .preview(0, None, vec!["bash".into(), "first\nsecond\nthird".into()]);
    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    let now = Instant::now();
    let history_len = app.history_len(60, now);
    let layout = app.screen_layout(Rect::new(0, 0, 60, 24), now);
    let history_start = app.visible_history_start(history_len, layout.history.height as usize);
    let prompt_line = app
        .history_lines(60, now)
        .iter()
        .position(|line| line_text(line).contains("ctrl+o to expand"))
        .unwrap();
    let row = layout.history.y + prompt_line.saturating_sub(history_start) as u16;

    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        2,
        row,
        &mut terminal,
    )
    .unwrap();
    app.handle_mouse_event(MouseEventKind::Up(MouseButton::Left), 2, row, &mut terminal)
        .unwrap();

    assert!(matches!(
        app.tool_calls.previews.get(&0),
        Some(ToolEntry { expanded: true, .. })
    ));
    assert_eq!(app.status, "tool output expanded");
}

#[test]
fn dragging_transcript_text_copies_on_mouse_release() {
    let copied = Arc::new(Mutex::new(Vec::new()));
    let mut app = test_app();
    app.clipboard = Box::new(RecordingClipboard {
        copied: copied.clone(),
    });
    app.record_inserted_entry(Entry::Assistant("hello world".into()));
    let mut terminal = Terminal::new(TestBackend::new(40, 18)).unwrap();
    let now = Instant::now();
    let history_len = app.history_len(40, now);
    let layout = app.screen_layout(Rect::new(0, 0, 40, 18), now);
    let history_start = app.visible_history_start(history_len, layout.history.height as usize);
    let full_lines = app.history_lines(40, now);
    let text_line = full_lines
        .iter()
        .position(|line| line_text(line).contains("hello world"))
        .unwrap();
    let row = layout.history.y + text_line.saturating_sub(history_start) as u16;

    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        1,
        row,
        &mut terminal,
    )
    .unwrap();
    app.handle_mouse_event(
        MouseEventKind::Drag(MouseButton::Left),
        5,
        row,
        &mut terminal,
    )
    .unwrap();
    app.handle_mouse_event(MouseEventKind::Up(MouseButton::Left), 5, row, &mut terminal)
        .unwrap();

    assert_eq!(*copied.lock().unwrap(), vec!["hello".to_string()]);
    assert_eq!(
        app.history.copy_notice().unwrap().message(),
        "5 chars copied"
    );
    assert!(app.history.text_selection().is_some());
}

#[test]
fn scrolling_during_drag_extends_selection_beyond_the_original_viewport() {
    let copied = Arc::new(Mutex::new(Vec::new()));
    let mut app = test_app();
    app.clipboard = Box::new(RecordingClipboard {
        copied: copied.clone(),
    });
    for index in 0..30 {
        app.record_inserted_entry(Entry::User(format!("message {index}")));
    }
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    let now = Instant::now();
    let history_len = app.history_len(40, now);
    let layout = app.screen_layout(Rect::new(0, 0, 40, 12), now);
    let history_start = app.visible_history_start(history_len, layout.history.height as usize);
    let full_lines = app.history_lines(40, now);
    let anchor_line = (history_start..history_start + layout.history.height as usize)
        .rev()
        .find(|&line| line_text(&full_lines[line]).contains("message"))
        .unwrap();
    let anchor_text = line_text(&full_lines[anchor_line]);
    let anchor_row = layout.history.y + (anchor_line - history_start) as u16;

    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        39,
        anchor_row,
        &mut terminal,
    )
    .unwrap();
    app.handle_mouse_event(MouseEventKind::ScrollUp, 0, layout.history.y, &mut terminal)
        .unwrap();
    assert!(app.history.text_selection().is_some());

    let scrolled_layout = app.screen_layout(Rect::new(0, 0, 40, 12), Instant::now());
    let scrolled_start =
        app.visible_history_start(history_len, scrolled_layout.history.height as usize);
    let focus_line = (scrolled_start..scrolled_start + scrolled_layout.history.height as usize)
        .find(|&line| line_text(&full_lines[line]).contains("message"))
        .unwrap();
    let focus_text = line_text(&full_lines[focus_line]);
    let focus_row = scrolled_layout.history.y + (focus_line - scrolled_start) as u16;
    app.handle_mouse_event(
        MouseEventKind::Drag(MouseButton::Left),
        0,
        focus_row,
        &mut terminal,
    )
    .unwrap();
    app.handle_mouse_event(
        MouseEventKind::Up(MouseButton::Left),
        0,
        focus_row,
        &mut terminal,
    )
    .unwrap();

    let copied = copied.lock().unwrap();
    assert_eq!(copied.len(), 1);
    assert!(copied[0].contains(anchor_text.trim()), "{}", copied[0]);
    assert!(copied[0].contains(focus_text.trim()), "{}", copied[0]);
}

#[test]
fn mermaid_copy_button_copies_source_instead_of_rendered_art() {
    let copied = Arc::new(Mutex::new(Vec::new()));
    let mut app = test_app();
    app.clipboard = Box::new(RecordingClipboard {
        copied: copied.clone(),
    });
    let source = "flowchart LR\nA[Parse] --> B[Render]";
    app.record_inserted_entry(Entry::Assistant(format!("```mermaid\n{source}\n```")));
    let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
    let now = Instant::now();
    let history_len = app.history_len(60, now);
    let layout = app.screen_layout(Rect::new(0, 0, 60, 18), now);
    let history_start = app.visible_history_start(history_len, layout.history.height as usize);
    let target = app.code_block_copy_targets(60).into_iter().next().unwrap();
    let column = target.columns.start as u16;
    let row = layout.history.y + target.line.saturating_sub(history_start) as u16;

    terminal.draw(|frame| app.draw(frame)).unwrap();
    let visible = terminal.backend().buffer();
    assert!(visible.content().iter().any(|cell| cell.symbol() == "M"));

    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        &mut terminal,
    )
    .unwrap();

    assert_eq!(*copied.lock().unwrap(), vec![source.to_string()]);
    assert!(app.history.text_selection().is_none());
}

#[test]
fn code_block_copy_button_hovers_and_copies_raw_contents() {
    let copied = Arc::new(Mutex::new(Vec::new()));
    let mut app = test_app();
    app.clipboard = Box::new(RecordingClipboard {
        copied: copied.clone(),
    });
    app.record_inserted_entry(Entry::Assistant(
        "```rust\nlet x = 1;\nprintln!(\"{x}\");\n```".into(),
    ));
    let mut terminal = Terminal::new(TestBackend::new(40, 18)).unwrap();
    let now = Instant::now();
    let history_len = app.history_len(40, now);
    let layout = app.screen_layout(Rect::new(0, 0, 40, 18), now);
    let history_start = app.visible_history_start(history_len, layout.history.height as usize);
    let target = app.code_block_copy_targets(40).into_iter().next().unwrap();
    let column = target.columns.start as u16;
    let row = layout.history.y + target.line.saturating_sub(history_start) as u16;

    app.handle_mouse_event(MouseEventKind::Moved, column, row, &mut terminal)
        .unwrap();
    assert_eq!(app.history.hovered_code_block_copy(), Some(target.line));
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let hovered_style = terminal.backend().buffer()[(column, row)].style();
    let expected_hovered_style = Theme::markdown_code_copy_button(/*hovered*/ true);
    assert_eq!(hovered_style.fg, expected_hovered_style.fg);
    assert_eq!(hovered_style.bg, expected_hovered_style.bg);

    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        &mut terminal,
    )
    .unwrap();

    assert_eq!(
        *copied.lock().unwrap(),
        vec!["let x = 1;\nprintln!(\"{x}\");".to_string()]
    );
    assert_eq!(
        app.history.copy_notice().unwrap().message(),
        "27 chars copied"
    );
    assert!(app.history.text_selection().is_none());
}
