use super::*;

#[test]
fn fullscreen_history_starts_at_bottom() {
    let mut app = test_app();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }

    let lines = app.active_lines_for_height(40, 12);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(rendered.contains("message 19"), "{rendered}");
    assert!(!rendered.contains("message 0"), "{rendered}");
}

#[test]
fn pageup_enters_manual_scroll_and_ctrl_g_returns_to_bottom() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }

    assert!(app
        .handle_history_key(
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            &mut terminal,
        )
        .unwrap());
    assert!(matches!(app.history_scroll, HistoryScroll::Manual { .. }));
    assert!(app.should_show_jump_to_bottom(40, 12, Instant::now()));

    assert!(app
        .handle_history_key(
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
            &mut terminal,
        )
        .unwrap());
    assert_eq!(app.history_scroll, HistoryScroll::Bottom);
    assert!(!app.should_show_jump_to_bottom(40, 12, Instant::now()));
}

#[test]
fn small_scroll_to_rendered_bottom_resumes_bottom_following() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }
    let history_len = app.history_len(40, Instant::now());
    let rendered_bottom_start =
        history_len.saturating_sub(app.history_height_for_screen(40, 12, Instant::now()));
    app.history_scroll = HistoryScroll::Manual {
        top_line: rendered_bottom_start.saturating_sub(1),
    };

    app.handle_mouse_event(MouseEventKind::ScrollDown, 0, 0, &mut terminal)
        .unwrap();

    assert_eq!(app.history_scroll, HistoryScroll::Bottom);
    assert!(!app.should_show_jump_to_bottom(40, 12, Instant::now()));
}

#[test]
fn pagedown_moves_manual_scroll_toward_bottom() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }

    app.handle_history_key(
        KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
        &mut terminal,
    )
    .unwrap();
    let before = match app.history_scroll {
        HistoryScroll::Manual { top_line } => top_line,
        HistoryScroll::Bottom => panic!("expected manual scroll"),
    };

    app.handle_history_key(
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        &mut terminal,
    )
    .unwrap();

    match app.history_scroll {
        HistoryScroll::Manual { top_line } => assert!(top_line > before),
        HistoryScroll::Bottom => {}
    }
}

#[test]
fn jump_button_renders_above_composer_only_when_scrolled_up() {
    let mut app = test_app();
    app.input = "draft".into();
    app.input_cursor = app.input_char_len();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }

    let bottom_lines = app
        .active_lines_for_height(40, 12)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    assert!(!bottom_lines
        .iter()
        .any(|line| line.contains("jump to bottom")));

    app.scroll_history_page_up(40, 12, Instant::now());
    let scrolled_lines = app
        .active_lines_for_height(40, 12)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    let button_index = scrolled_lines
        .iter()
        .position(|line| line.contains("jump to bottom"))
        .unwrap();
    let input_index = scrolled_lines
        .iter()
        .position(|line| line.trim_end() == "draft")
        .unwrap();

    assert!(button_index < input_index, "{scrolled_lines:#?}");
}

#[test]
fn activity_row_is_always_reserved() {
    let mut app = test_app();
    let area = Rect::new(0, 0, 40, 12);
    let idle = app.screen_layout(area, Instant::now());

    app.running = true;
    let loading = app.screen_layout(area, Instant::now());

    assert_eq!(idle.history, loading.history);
    assert_eq!(idle.activity, loading.activity);
    assert_eq!(idle.history.bottom(), idle.activity.unwrap().y);
}

#[test]
fn spinner_and_jump_button_share_activity_row_with_jump_right_aligned() {
    let mut app = test_app();
    app.running = true;
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }
    app.scroll_history_page_up(40, 12, Instant::now());

    let layout = app.screen_layout(Rect::new(0, 0, 40, 12), Instant::now());
    let activity = layout.activity.unwrap();
    let button = layout.jump_to_bottom.unwrap();
    let line = app.activity_line(40, Instant::now(), /*show_jump_to_bottom*/ true);
    let rendered = line_text(&line);

    assert_eq!(activity.y, button.y);
    assert_eq!(
        button.x.saturating_add(button.width),
        activity.x.saturating_add(activity.width)
    );
    assert!(rendered.starts_with('⠋'), "{rendered:?}");
    assert!(
        rendered.ends_with("↓ jump to bottom  ctrl+g"),
        "{rendered:?}"
    );
}

#[test]
fn compact_jump_button_renders_on_narrow_terminals() {
    let app = test_app();

    assert!(line_text(&app.jump_to_bottom_line(16)).contains("bottom ctrl+g"));
    assert!(line_text(&app.jump_to_bottom_line(40)).contains("ctrl+g"));
}

#[test]
fn mouse_wheel_scrolls_history_and_clicking_jump_button_returns_to_bottom() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }

    app.handle_mouse_event(MouseEventKind::ScrollUp, 0, 0, &mut terminal)
        .unwrap();
    assert!(matches!(app.history_scroll, HistoryScroll::Manual { .. }));

    let layout = app.screen_layout(Rect::new(0, 0, 40, 12), Instant::now());
    let button = layout.jump_to_bottom.unwrap();
    assert!(button.x > 0);
    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        button.x,
        button.y,
        &mut terminal,
    )
    .unwrap();

    assert_eq!(app.history_scroll, HistoryScroll::Bottom);
}

#[test]
fn scrollbar_hides_until_scroll_or_hover() {
    let mut app = test_app();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }
    let height = 12;
    let mut terminal = Terminal::new(TestBackend::new(40, height)).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();
    let rows = (0..height)
        .map(|row| buffer_row_text(terminal.backend().buffer(), row))
        .collect::<Vec<_>>();

    assert!(!rows.iter().any(|row| row.ends_with('█')), "{rows:#?}");
}

#[test]
fn scrollbar_renders_briefly_after_mouse_wheel_scroll() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }

    app.handle_mouse_event(MouseEventKind::ScrollUp, 0, 0, &mut terminal)
        .unwrap();

    assert!(app.should_render_history_scrollbar(Instant::now()));
}

#[test]
fn scrollbar_renders_while_hovered() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }
    let layout = app.screen_layout(Rect::new(0, 0, 40, 12), Instant::now());
    let scrollbar = layout.history_scrollbar.unwrap();

    app.handle_mouse_event(
        MouseEventKind::Moved,
        scrollbar.rect.x,
        scrollbar.rect.y,
        &mut terminal,
    )
    .unwrap();

    assert!(app.should_render_history_scrollbar(Instant::now()));
}

#[test]
fn dragging_scrollbar_updates_history_scroll() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    for index in 0..30 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }
    app.scroll_history_page_up(40, 12, Instant::now());
    app.scroll_history_page_up(40, 12, Instant::now());
    app.scroll_history_page_up(40, 12, Instant::now());
    app.reveal_history_scrollbar(Instant::now());
    let layout = app.screen_layout(Rect::new(0, 0, 40, 12), Instant::now());
    let scrollbar = layout.history_scrollbar.unwrap();
    let thumb_row = (scrollbar.rect.y..scrollbar.rect.y.saturating_add(scrollbar.rect.height))
        .find(|row| {
            matches!(
                scrollbar.begin_drag(*row),
                HistoryScrollbarDrag::Thumb { .. }
            )
        })
        .unwrap();

    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        scrollbar.rect.x,
        thumb_row,
        &mut terminal,
    )
    .unwrap();
    assert!(matches!(
        app.history_scrollbar_drag,
        Some(HistoryScrollbarDrag::Thumb { .. })
    ));
    let before = match app.history_scroll {
        HistoryScroll::Manual { top_line } => top_line,
        HistoryScroll::Bottom => panic!("expected manual scroll"),
    };

    let drag_row = scrollbar.rect.y;

    app.handle_mouse_event(
        MouseEventKind::Drag(MouseButton::Left),
        scrollbar.rect.x,
        drag_row,
        &mut terminal,
    )
    .unwrap();
    let after = match app.history_scroll {
        HistoryScroll::Manual { top_line } => top_line,
        HistoryScroll::Bottom => usize::MAX,
    };
    assert!(after < before, "before={before} after={after}");

    app.handle_mouse_event(
        MouseEventKind::Up(MouseButton::Left),
        scrollbar.rect.x,
        drag_row,
        &mut terminal,
    )
    .unwrap();
    assert_eq!(app.history_scrollbar_drag, None);
}

#[test]
fn clicking_scrollbar_track_jumps_history_scroll() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    for index in 0..30 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }
    let layout = app.screen_layout(Rect::new(0, 0, 40, 12), Instant::now());
    let scrollbar = layout.history_scrollbar.unwrap();
    app.reveal_history_scrollbar(Instant::now());

    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        scrollbar.rect.x,
        scrollbar.rect.y.saturating_add(scrollbar.rect.height - 1),
        &mut terminal,
    )
    .unwrap();

    assert_eq!(app.history_scroll, HistoryScroll::Bottom);
}

#[test]
fn clicking_hidden_scrollbar_does_not_scroll_history() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    for index in 0..30 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }
    app.scroll_history_page_up(40, 12, Instant::now());
    let layout = app.screen_layout(Rect::new(0, 0, 40, 12), Instant::now());
    let scrollbar = layout.history_scrollbar.unwrap();
    let before = app.history_scroll;

    app.handle_mouse_event(
        MouseEventKind::Down(MouseButton::Left),
        scrollbar.rect.x,
        scrollbar.rect.y.saturating_add(scrollbar.rect.height - 1),
        &mut terminal,
    )
    .unwrap();

    assert_eq!(app.history_scroll, before);
    assert_eq!(app.history_scrollbar_drag, None);
}

#[test]
fn clamping_bottom_scroll_preserves_scrollbar_hover() {
    let mut app = test_app();
    for index in 0..30 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }
    let layout = app.screen_layout(Rect::new(0, 0, 40, 12), Instant::now());
    let scrollbar = layout.history_scrollbar.unwrap();
    app.update_history_scrollbar_hover(
        layout.history_scrollbar,
        scrollbar.rect.x,
        scrollbar.rect.y,
    );

    app.clamp_history_scroll(40, 12, Instant::now());

    assert!(app.history_scrollbar_hovered);
    assert!(app.should_render_history_scrollbar(Instant::now()));
}

#[test]
fn manual_scroll_preserves_top_line_when_new_output_arrives() {
    let mut app = test_app();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }
    app.scroll_history_page_up(40, 12, Instant::now());
    let before_len = app.history_len(40, Instant::now());
    let before_start = app.visible_history_start(before_len, 6);

    app.push_transcript_entry(Entry::Assistant("new output".into()));
    let after_len = app.history_len(40, Instant::now());
    let after_start = app.visible_history_start(after_len, 6);

    assert_eq!(after_start, before_start);
}

#[test]
fn bottom_scroll_follows_new_output() {
    let mut app = test_app();
    for index in 0..20 {
        app.push_transcript_entry(Entry::User(format!("message {index}")));
    }
    let before_len = app.history_len(40, Instant::now());
    let before_start = app.visible_history_start(before_len, 6);

    app.push_transcript_entry(Entry::Assistant("new output".into()));
    let after_len = app.history_len(40, Instant::now());
    let after_start = app.visible_history_start(after_len, 6);

    assert!(after_start > before_start);
}

#[test]
fn repeated_statusline_frames_render_once() {
    let mut app = test_app();

    for _ in 0..10_000 {
        app.statusline_lines(120);
    }

    assert_eq!(app.statusline.render_count(), 1);
}

#[test]
fn hidden_reasoning_shows_thinking_placeholder() {
    let mut app = test_app();
    app.active_turn_show_reasoning_output = false;
    app.record_agent_event(AgentEvent::StepStarted(1));

    let thinking = app
        .history_live_lines(60, Instant::now())
        .into_iter()
        .find(|line| line_text(line).contains("Thinking..."))
        .unwrap();
    assert_eq!(thinking.spans[1].style, StreamKind::Reasoning.style());

    app.reset_streams();
    assert!(!app.hidden_reasoning_active);
}
