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
fn spinner_overlays_last_history_row_without_reducing_layout_height() {
    let mut app = test_app();
    let area = Rect::new(0, 0, 40, 12);
    let idle = app.screen_layout(area, Instant::now());

    app.running = true;
    let loading = app.screen_layout(area, Instant::now());

    assert_eq!(idle.history, loading.history);
    assert_eq!(idle.activity, None);
    let activity = loading.activity.unwrap();
    assert_eq!(activity.y, loading.history.bottom().saturating_sub(1));
    assert!(activity.width < loading.history.width);
}

#[test]
fn spinner_offsets_bottom_following_but_not_manual_scroll() {
    let mut app = test_app();
    app.running = true;

    assert_eq!(app.visible_history_window(20, 10), (11, 9));

    app.history_scroll = HistoryScroll::Manual { top_line: 3 };
    assert_eq!(app.visible_history_window(20, 10), (3, 10));
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
    assert_eq!(activity.y, button.y);
    assert_eq!(activity.x, layout.history.x);
    assert!(activity.right() < button.x);
    assert_eq!(button.right(), layout.history.right());
}

#[test]
fn bottom_following_renders_last_message_above_spinner_overlay() {
    let mut app = test_app();
    app.running = true;
    for index in 0..20 {
        app.push_transcript_entry(Entry::Assistant(format!("message {index}")));
    }
    let width = 40;
    let height = 12;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let layout = app.screen_layout(Rect::new(0, 0, width, height), Instant::now());
    let activity = layout.activity.unwrap();
    let rows = (0..height)
        .map(|row| buffer_row_text(terminal.backend().buffer(), row))
        .collect::<Vec<_>>();
    assert!(
        rows[..activity.y as usize]
            .iter()
            .any(|row| row.contains("message 19")),
        "{rows:#?}"
    );
    assert!(
        !rows[activity.y as usize].contains("message 19"),
        "{rows:#?}"
    );
}

#[test]
fn jump_button_preserves_uncovered_content_on_last_scrolled_row() {
    let mut app = test_app();
    let width = 40;
    let height = 12;
    let now = Instant::now();
    for index in 0..30 {
        app.push_transcript_entry(Entry::Assistant(format!(
            "message {index:02} remains visible across this row"
        )));
    }
    let history_len = app.history_len(width as usize, now);
    let layout = app.screen_layout(Rect::new(0, 0, width, height), now);
    let history_height = layout.history.height as usize;
    let top_line = (0..history_len.saturating_sub(history_height))
        .find(|top_line| {
            app.visible_history_lines(width as usize, now, *top_line, history_height)
                .last()
                .is_some_and(|line| !line_text(line).trim().is_empty())
        })
        .unwrap();
    app.history_scroll = HistoryScroll::Manual { top_line };
    let layout = app.screen_layout(Rect::new(0, 0, width, height), now);
    let button = layout.jump_to_bottom.unwrap();
    let expected = app
        .visible_history_lines(width as usize, now, top_line, history_height)
        .last()
        .map(line_text)
        .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let rendered = buffer_row_text(terminal.backend().buffer(), button.y);
    let preserved_width = button.x as usize;
    assert!(
        !expected[..preserved_width].trim().is_empty(),
        "{expected:?}"
    );
    assert_eq!(&rendered[..preserved_width], &expected[..preserved_width]);
    assert!(
        rendered.ends_with("↓ jump to bottom  ctrl+g"),
        "{rendered:?}"
    );
}

#[test]
fn activity_background_fills_every_row_below_the_spinner() {
    let mut app = test_app();
    let width = 40;
    let height = 12;
    app.running = true;
    app.push_transcript_entry(test_tool_entry(
        true,
        &[
            "tool line 0",
            "tool line 1",
            "tool line 2",
            "tool line 3",
            "tool line 4",
            "tool line 5",
            "tool line 6",
            "tool line 7",
            "tool line 8",
            "tool line 9",
        ],
    ));
    let layout = app.screen_layout(Rect::new(0, 0, width, height), Instant::now());
    let background = layout.activity_background.unwrap();
    let rail = layout.activity_rail.unwrap();
    let activity = layout.activity.unwrap();
    let scrollbar = layout.history_scrollbar.unwrap();
    app.reveal_history_scrollbar(Instant::now());
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    assert_eq!(rail, Rect::new(0, rail.y, width, 1));
    assert_eq!(
        background,
        Rect::new(0, rail.y, width, height.saturating_sub(rail.y))
    );
    let buffer = terminal.backend().buffer();
    let rail_background = Theme::activity_rail().bg.unwrap();
    assert_ne!(
        buffer[(rail.x, rail.y.saturating_sub(1))].bg,
        rail_background
    );
    for row in background.y..background.bottom() {
        for column in background.x..background.right() {
            assert_eq!(buffer[(column, row)].bg, rail_background);
        }
    }
    for column in activity.right()..scrollbar.rect.x {
        assert_eq!(buffer[(column, rail.y)].symbol(), " ");
    }
    assert_eq!(buffer[(scrollbar.rect.x, rail.y)].symbol(), "█");
}

#[test]
fn activity_rail_clears_inherited_text_modifiers() {
    let mut app = test_app();
    let width = 40;
    let height = 12;
    for index in 0..20 {
        app.push_transcript_entry(Entry::Notice(format!("italic status {index}")));
    }
    app.running = true;
    let layout = app.screen_layout(Rect::new(0, 0, width, height), Instant::now());
    let rail = layout.activity_rail.unwrap();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let buffer = terminal.backend().buffer();
    assert!(buffer[(rail.x, rail.y.saturating_sub(1))]
        .modifier
        .contains(Modifier::ITALIC));
    for column in rail.x..rail.right() {
        assert!(!buffer[(column, rail.y)].modifier.contains(Modifier::ITALIC));
    }
}

#[test]
fn jump_button_uses_activity_rail_background() {
    let mut app = test_app();
    let width = 40;
    let height = 12;
    app.push_transcript_entry(test_tool_entry(
        true,
        &[
            "tool line 0",
            "tool line 1",
            "tool line 2",
            "tool line 3",
            "tool line 4",
            "tool line 5",
            "tool line 6",
            "tool line 7",
            "tool line 8",
            "tool line 9",
        ],
    ));
    for index in 0..20 {
        app.push_transcript_entry(Entry::Assistant(format!("later message {index}")));
    }
    app.history_scroll = HistoryScroll::Manual { top_line: 0 };
    let layout = app.screen_layout(Rect::new(0, 0, width, height), Instant::now());
    let button = layout.jump_to_bottom.unwrap();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let buffer = terminal.backend().buffer();
    let rail_background = Theme::activity_rail().bg.unwrap();
    assert_ne!(
        buffer[(button.x.saturating_sub(1), button.y)].bg,
        rail_background
    );
    for column in button.x..button.right() {
        assert_eq!(buffer[(column, button.y)].bg, rail_background);
    }
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
        scrollbar
            .rect
            .y
            .saturating_add(scrollbar.rect.height.saturating_sub(2)),
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
    app.record_agent_event(ViewModelEvent::StepStarted(1));

    let thinking = app
        .history_live_lines(60, Instant::now())
        .into_iter()
        .find(|line| line_text(line).contains("Thinking..."))
        .unwrap();
    assert_eq!(thinking.spans[1].style, StreamKind::Reasoning.style());

    app.reset_streams();
    assert!(!app.hidden_reasoning_active);
}

#[test]
fn started_tool_display_ignores_late_argument_previews() {
    let mut app = test_app();

    app.record_agent_event(ViewModelEvent::ToolStarted {
        display_lines: vec!["edit_file src/main.rs".into()],
    });
    app.record_agent_event(ViewModelEvent::ToolCallUpdated {
        display_lines: vec!["edit_file".into()],
    });

    assert_eq!(
        app.pending_tool_call
            .as_ref()
            .map(|tool| tool.display_lines.as_slice()),
        Some(["edit_file src/main.rs".to_string()].as_slice())
    );
}
