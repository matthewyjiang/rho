use super::{
    command_palette::{complete_slash_command, slash_command_args},
    message_history::{recovered_history_tail, transcript_entries_from_messages},
    paste_burst::{normalize_paste, previous_word_boundary},
    render::entry_lines,
    tool_output_ui::expandable_tool_entry,
    transcript_events::final_answer_delta,
    *,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{backend::TestBackend, style::Color, Terminal};
use rho_providers::credentials::{
    save_codex_tokens, save_provider_api_key, CredentialError, CredentialResult,
    MemoryCredentialStore,
};
use std::sync::atomic::AtomicBool;

#[path = "tests/activity_phase_tests.rs"]
mod activity_phase_tests;
#[path = "tests/herdr_state_tests.rs"]
mod herdr_state_tests;
#[path = "tests/input_editing_tests.rs"]
mod input_editing_tests;
#[path = "tests/layout_tests.rs"]
mod layout_tests;
#[path = "tests/mouse_tests.rs"]
mod mouse_tests;
#[path = "tests/questionnaire_interaction_tests.rs"]
mod questionnaire_interaction_tests;
#[path = "tests/shell_composer_tests.rs"]
mod shell_composer_tests;
#[path = "tests/subagent_notification_tests.rs"]
mod subagent_notification_tests;
#[path = "tests/usage_tests.rs"]
mod usage_tests;

#[derive(Debug)]
struct FailingCredentialStore;

impl CredentialStore for FailingCredentialStore {
    fn get_secret(&self, _account: &str) -> CredentialResult<Option<String>> {
        Err(CredentialError::StoreUnavailable("test failure".into()))
    }

    fn set_secret(&self, _account: &str, _secret: &str) -> CredentialResult<()> {
        unreachable!()
    }

    fn delete_secret(&self, _account: &str) -> CredentialResult<bool> {
        unreachable!()
    }
}

pub(super) fn test_bootstrap() -> TuiBootstrap {
    TuiBootstrap {
        runtime: RuntimeModelView {
            cwd: PathBuf::from("/tmp/project"),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            model_aliases: Default::default(),
            reasoning: ReasoningLevel::Low,
            reasoning_source: ReasoningRequestSource::PersistedOrDefault,
            permission_mode: PermissionMode::Auto,
            show_reasoning_output: true,
            auth: "api-key".into(),
            internal_agents: Default::default(),
            favorite_models: Vec::new(),
            max_tool_output_lines: 10,
            keybindings: Keybindings::default(),
            prompt_templates: Default::default(),
        },
        session: SessionBootstrap {
            session_id: None,
            recovered_messages: Vec::new(),
            open_resume_picker: false,
        },
        services: ApplicationServices {
            config_repository: ConfigRepository::temporary_for_tests().unwrap(),
            auth_unavailable: None,
            update_notice: None,
            pending_update_notice: None,
            diagnostics: crate::diagnostics::test_diagnostics("openai", "gpt-test"),
            herdr: HerdrReporter::default(),
        },
    }
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn buffer_row_text(buffer: &ratatui::buffer::Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

fn test_tool_entry(ok: bool, display_lines: &[&str]) -> Entry {
    Entry::Tool(ToolEntry {
        state: ToolEntryState::Finished {
            ok,
            display_style: ToolDisplayStyle::file_or_command(),
        },
        display_lines: display_lines.iter().map(|line| (*line).into()).collect(),
        expanded: false,
        image: None,
    })
}

pub(super) fn test_app() -> App {
    let store = Arc::new(MemoryCredentialStore::default());
    save_provider_api_key(store.as_ref(), "openai", "sk-test").unwrap();
    App::new_with_credentials(
        test_bootstrap(),
        store,
        crate::herdr::HerdrGraphicsCapability::NotHerdr,
    )
}

#[test]
fn info_command_uses_runtime_diagnostics() {
    let mut app = test_app();

    app.execute_info_command().unwrap();

    assert!(matches!(app.transcript.last(), Some(Entry::RuntimeInfo(_))));
    assert_eq!(app.status, "runtime info");
}

#[test]
fn interrupt_during_tool_ends_turn_immediately() {
    let mut app = test_app();
    let interrupt_requested = AtomicBool::new(false);
    let tool_call_active = AtomicBool::new(true);

    let control = app.request_running_interrupt(&interrupt_requested, &tool_call_active);

    assert!(interrupt_requested.load(Ordering::SeqCst));
    assert!(matches!(control, StreamControl::Interrupt));
    assert_eq!(app.status, "interrupting tool");
}

#[test]
fn sanitizes_generated_session_title() {
    assert_eq!(
        session_title::sanitize_session_title("\"Implement resume picker.\""),
        Some("Implement resume picker".into())
    );
    assert_eq!(session_title::sanitize_session_title("\n\n"), None);
}

#[test]
fn title_model_defaults_to_main_model() {
    let app = test_app();

    assert_eq!(
        app.internal_agent_model_selection(crate::agent::SESSION_TITLE_AGENT_ID),
        crate::config::InternalAgentModelConfig::new(
            "openai".into(),
            "gpt-5.5".into(),
            "api-key".into()
        )
    );
}

#[test]
fn transcript_and_status_mutations_do_not_require_a_terminal() {
    let mut app = test_app();

    app.insert_entry(&Entry::Assistant("hello".into()));
    app.insert_entry(&Entry::Assistant(" world".into()));
    app.notify_status("ready");
    app.notify_status("ready");

    assert!(matches!(
        app.transcript.as_slice(),
        [Entry::Assistant(answer), Entry::Notice(status)]
            if answer == "hello world" && status == "ready"
    ));
    assert_eq!(app.status, "ready");
}

#[test]
fn transcript_entries_render_without_prefix_labels() {
    let entries = [
        Entry::User("hello?".into()),
        Entry::Assistant("hi".into()),
        test_tool_entry(true, &["read_file", "read src/main.rs"]),
        Entry::Notice("note".into()),
        Entry::Error("bad".into()),
    ];

    let rendered = entries
        .iter()
        .flat_map(|entry| entry_lines(entry, 40, 10))
        .map(|line| line_text(&line))
        .collect::<Vec<_>>()
        .join("\n");

    for label in ["you>", "rho>", "reasoning>", "tool:", "notice>", "error>"] {
        assert!(
            !rendered.contains(label),
            "rendered label {label}: {rendered}"
        );
    }
}

#[test]
fn recovered_history_tail_limits_initial_redraw() {
    let entries = (0..10)
        .map(|index| Entry::User(format!("message {index}")))
        .collect::<Vec<_>>();

    let (omitted, visible) = recovered_history_tail(&entries, 80, 9, 10);

    assert_eq!(omitted, 7);
    assert!(matches!(visible.as_slice(), [
            Entry::User(a),
            Entry::User(b),
            Entry::User(c),
        ] if a == "message 7" && b == "message 8" && c == "message 9"));
}

#[test]
fn key_event_paste_burst_collapses_through_common_paste_path() {
    let start = Instant::now();
    let mut app = test_app();

    for (index, ch) in "alpha\nbeta".chars().enumerate() {
        let key = if ch == '\n' {
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        } else {
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
        };
        assert!(app.handle_paste_burst_key_at(key, start + Duration::from_millis(index as u64)));
    }
    app.flush_pending_paste_burst();

    assert_eq!(app.input, "[ pasted: 2 lines ]");
    assert_eq!(app.expanded_input(), "alpha\nbeta");
}

#[test]
fn idle_key_event_text_is_inserted_without_paste_marker() {
    let start = Instant::now();
    let mut app = test_app();

    assert!(
        app.handle_paste_burst_key_at(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE), start)
    );
    assert!(!app.handle_paste_burst_key_at(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        start + Duration::from_millis(20)
    ));

    assert_eq!(app.input, "a");
    assert!(app.paste_segments.is_empty());
}

#[test]
fn single_character_fast_enter_is_buffered_as_paste() {
    let start = Instant::now();
    let mut app = test_app();

    assert!(
        app.handle_paste_burst_key_at(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE), start)
    );
    assert!(app.handle_paste_burst_key_at(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        start + Duration::from_millis(1)
    ));

    assert_eq!(app.input, "");
    assert!(app.paste_segments.is_empty());
}

#[test]
fn pasted_multiline_input_collapses_to_marker_and_expands() {
    let mut app = test_app();

    app.insert_pasted_input_text("alpha\nbeta\ngamma");

    assert_eq!(app.input, "[ pasted: 3 lines ]");
    assert_eq!(app.input_cursor, app.input.chars().count());
    assert_eq!(app.expanded_input(), "alpha\nbeta\ngamma");
}

#[test]
fn pasted_single_line_input_stays_literal_until_large() {
    let mut app = test_app();

    app.insert_pasted_input_text("hello world");

    assert_eq!(app.input, "hello world");
    assert!(app.paste_segments.is_empty());
    assert_eq!(app.expanded_input(), "hello world");
}

#[test]
fn paste_segments_shift_after_edits_before_marker() {
    let mut app = test_app();
    app.insert_pasted_input_text("alpha\nbeta");
    app.input_cursor = 0;
    app.insert_input_text("prefix ");

    assert_eq!(app.input, "prefix [ pasted: 2 lines ]");
    assert_eq!(app.expanded_input(), "prefix alpha\nbeta");
}

#[test]
fn queued_pasted_prompt_keeps_marker_when_recalled_for_editing() {
    let mut app = test_app();
    app.insert_pasted_input_text("alpha\nbeta");
    let queued = QueuedPrompt {
        prompt: app.expanded_input(),
        display_prompt: app.input.clone(),
        paste_segments: app.paste_segments.clone(),
    };
    app.input.clear();
    app.paste_segments.clear();
    app.queued_prompts.push_back(queued);

    assert!(app.handle_pending_input_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT,)));
    assert_eq!(app.input, "[ pasted: 2 lines ]");
    assert_eq!(app.expanded_input(), "alpha\nbeta");
}

#[test]
fn queued_pasted_prompt_preserves_leading_space_segment_offsets() {
    let mut app = test_app();
    app.insert_input_text(" ");
    app.insert_pasted_input_text("alpha\nbeta");
    let queued = QueuedPrompt {
        prompt: app.expanded_input().trim().to_string(),
        display_prompt: app.input.clone(),
        paste_segments: app.paste_segments.clone(),
    };
    app.input.clear();
    app.paste_segments.clear();
    app.queued_prompts.push_back(queued);

    assert!(app.handle_pending_input_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT,)));
    assert_eq!(app.input, " [ pasted: 2 lines ]");
    assert_eq!(app.expanded_input().trim(), "alpha\nbeta");
}

#[test]
fn slash_command_args_can_keep_collapsed_display_separate_from_expanded_prompt() {
    let mut app = test_app();
    app.insert_input_text("/skill:test ");
    app.insert_pasted_input_text("alpha\nbeta");

    let expanded_input = app.expanded_input();
    assert_eq!(slash_command_args(&expanded_input).trim(), "alpha\nbeta");
    assert_eq!(slash_command_args(&app.input).trim(), "[ pasted: 2 lines ]");
}

#[test]
fn normalize_paste_converts_carriage_returns() {
    assert_eq!(normalize_paste("a\r\nb\rc"), "a\nb\nc");
}

#[test]
fn recovered_session_messages_become_transcript_entries() {
    let entries = transcript_entries_from_messages(
        &[
            Message::System("system".into()),
            Message::User(vec![
                ContentBlock::Text("hello".into()),
                ContentBlock::Image(ImageContent {
                    data: "aW1n".into(),
                    mime_type: "image/png".into(),
                }),
            ]),
            Message::Assistant(vec![ContentBlock::Text("hi".into())]),
            Message::Assistant(vec![ContentBlock::ToolCall(rho_tools::tool::ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            })]),
            Message::ToolResult(rho_tools::tool::ToolResult {
                id: "call_1".into(),
                ok: false,
                content: "missing file".into(),
            }),
        ],
        std::path::Path::new(""),
    );

    assert!(matches!(entries[0], Entry::User(ref text) if text == "hello\n[image: image/png 3 B]"));
    assert!(matches!(entries[1], Entry::Assistant(ref text) if text == "hi"));
    assert!(matches!(
        entries[2],
        Entry::Tool(ToolEntry {
            state: ToolEntryState::Finished {
                ok: false,
                display_style: ToolDisplayStyle::FileOrCommand,
            },
            ref display_lines,
            ..
        }) if display_lines == &vec!["read_file src/main.rs".to_string()]
    ));
    let lines = entry_lines(&entries[2], 40, 10);
    assert_eq!(lines[1].spans[0].style.fg, Some(Color::White));
    assert_eq!(lines[1].spans[0].style.bg, Some(Color::Red));
    assert!(!lines[1].spans[0].style.add_modifier.contains(Modifier::DIM));
}

#[test]
fn bash_tool_block_shows_command() {
    let lines = entry_lines(
        &test_tool_entry(true, &["bash", "cargo test", "ignored output"]),
        40,
        10,
    );
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(rendered.contains("bash"));
    assert!(rendered.contains("cargo test"));
    assert!(!rendered.contains("tool:"));
}

#[test]
fn read_file_tool_block_shows_file_name_only() {
    let lines = entry_lines(
        &test_tool_entry(true, &["read_file", "src/main.rs"]),
        40,
        10,
    );
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(rendered.contains("read_file"));
    assert!(rendered.contains("src/main.rs"));
}

#[test]
fn skill_tool_block_shows_single_magenta_status_line() {
    let lines = entry_lines(
        &Entry::Tool(ToolEntry {
            state: ToolEntryState::Finished {
                ok: true,
                display_style: ToolDisplayStyle::skill(),
            },
            display_lines: vec!["skill caveman".into()],
            expanded: false,
            image: None,
        }),
        40,
        10,
    );
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert_eq!(lines[1].spans[0].style.fg, Some(Color::White));
    assert_eq!(lines[1].spans[0].style.bg, Some(Color::Magenta));
    assert!(!lines[1].spans[0].style.add_modifier.contains(Modifier::DIM));
    assert!(rendered.contains("skill caveman"));
    assert_eq!(rendered.matches("skill").count(), 1);
}

#[test]
fn skill_tool_block_uses_subtle_red_failure_background() {
    let lines = entry_lines(
        &Entry::Tool(ToolEntry {
            state: ToolEntryState::Finished {
                ok: false,
                display_style: ToolDisplayStyle::skill(),
            },
            display_lines: vec!["unknown skill".into()],
            expanded: false,
            image: None,
        }),
        40,
        10,
    );

    assert_eq!(lines[1].spans[0].style.fg, Some(Color::White));
    assert_eq!(lines[1].spans[0].style.bg, Some(Color::Red));
    assert!(!lines[1].spans[0].style.add_modifier.contains(Modifier::DIM));
}

#[test]
fn read_file_tool_block_shows_line_range_label() {
    let lines = entry_lines(
        &test_tool_entry(true, &["read_file", "src/file.rs:10-24"]),
        40,
        10,
    );
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(rendered.contains("read_file"));
    assert!(rendered.contains("src/file.rs:10-24"));
}
#[test]
fn tool_block_truncates_multiline_output_with_expand_prompt() {
    let lines = entry_lines(
        &test_tool_entry(true, &["bash", "line 1\nline 2\nline 3"]),
        40,
        2,
    );
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(rendered.contains("bash"));
    assert!(rendered.contains("line 1"));
    assert!(!rendered.contains("line 2"));
    assert!(rendered.contains("... 2 more lines, ctrl+o to expand"));
}

#[test]
fn expanded_tool_block_shows_full_multiline_output() {
    let mut entry = test_tool_entry(true, &["bash", "line 1\nline 2\nline 3"]);
    let Entry::Tool(tool) = &mut entry else {
        panic!("expected tool entry");
    };
    tool.expanded = true;

    let lines = entry_lines(&entry, 40, 2);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(rendered.contains("line 1"));
    assert!(rendered.contains("line 2"));
    assert!(rendered.contains("line 3"));
    assert!(rendered.contains("ctrl+o to collapse"));
}

#[test]
fn untruncated_tool_block_does_not_show_expand_prompt() {
    let lines = entry_lines(&test_tool_entry(true, &["bash", "line 1"]), 40, 2);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(!rendered.contains("ctrl+o"));
}

#[test]
fn toggling_latest_truncated_tool_collapses_previous_tool() {
    let mut app = test_app();
    app.info.runtime.max_tool_output_lines = 1;
    app.transcript = vec![
        test_tool_entry(true, &["first", "a\nb"]),
        test_tool_entry(true, &["second", "c\nd"]),
    ];
    if let Entry::Tool(tool) = &mut app.transcript[0] {
        tool.expanded = true;
    }

    let index = app
        .transcript
        .iter()
        .rposition(|entry| expandable_tool_entry(entry, app.info.runtime.max_tool_output_lines))
        .unwrap();
    for entry in &mut app.transcript {
        if let Entry::Tool(tool) = entry {
            tool.expanded = false;
        }
    }
    if let Entry::Tool(tool) = &mut app.transcript[index] {
        tool.expanded = true;
    }

    assert!(matches!(
        app.transcript[0],
        Entry::Tool(ToolEntry {
            expanded: false,
            ..
        })
    ));
    assert!(matches!(
        app.transcript[1],
        Entry::Tool(ToolEntry { expanded: true, .. })
    ));
}

#[test]
fn final_answer_delta_handles_unstreamed_suffix_and_mismatch() {
    assert_eq!(
        final_answer_delta("", "final"),
        FinalAnswerDelta::Append("final")
    );
    assert_eq!(
        final_answer_delta("hello", "hello world"),
        FinalAnswerDelta::Append(" world")
    );
    assert_eq!(final_answer_delta("hello", "hello"), FinalAnswerDelta::None);
    assert_eq!(
        final_answer_delta("hello", "goodbye"),
        FinalAnswerDelta::Mismatch
    );
}

#[test]
fn final_answer_mismatch_replaces_transcript_without_duplicating_entry() {
    let mut app = test_app();
    app.push_transcript_entry(Entry::Assistant("streamed".into()));

    app.replace_current_turn_assistant_transcript("final");

    assert!(matches!(
        app.transcript.as_slice(),
        [Entry::Assistant(text)] if text == "final"
    ));
}

#[test]
fn final_answer_mismatch_replaces_transcript_with_empty_answer() {
    let mut app = test_app();
    app.push_transcript_entry(Entry::Assistant("streamed".into()));

    app.replace_current_turn_assistant_transcript("");

    assert!(matches!(
        app.transcript.as_slice(),
        [Entry::Assistant(text)] if text.is_empty()
    ));
}

#[test]
fn final_answer_mismatch_replaces_interleaved_current_turn_assistant_fragments() {
    let mut app = test_app();
    app.push_transcript_entry(Entry::User("prompt".into()));
    app.current_turn_start = Some(app.transcript.len());
    app.push_transcript_entry(Entry::Assistant("hel".into()));
    app.push_transcript_entry(Entry::Reasoning("thinking".into()));
    app.push_transcript_entry(Entry::Assistant("lo".into()));

    app.replace_current_turn_assistant_transcript("goodbye");

    assert!(matches!(
        app.transcript.as_slice(),
        [Entry::User(_), Entry::Assistant(text), Entry::Reasoning(_)] if text == "goodbye"
    ));
}

#[test]
fn active_lines_do_not_render_pending_stream_text() {
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.streams.assistant_stream.push_delta("hello");
    app.streams.reasoning_stream.push_delta("thinking");
    let lines = app.active_lines(40);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(rendered.contains("starting"), "{rendered}");
    assert!(!rendered.contains("hello"), "{rendered}");
    assert!(!rendered.contains("thinking"), "{rendered}");
}

#[test]
fn input_divider_style_tracks_reasoning_level() {
    let mut app = test_app();
    app.input = "hello".into();

    app.info.runtime.reasoning = ReasoningLevel::Off;
    let off_lines = app.active_lines(20);
    let off_divider = off_lines
        .iter()
        .find(|line| line_text(line) == "────────────────────")
        .unwrap();
    let off_style = off_divider.style;

    app.info.runtime.reasoning = ReasoningLevel::High;
    let high_lines = app.active_lines(20);
    let divider_indices = high_lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line_text(line) == "────────────────────").then_some(index))
        .collect::<Vec<_>>();
    let input_index = high_lines
        .iter()
        .position(|line| line_text(line) == "hello")
        .unwrap();
    let composer_top_divider = divider_indices
        .iter()
        .copied()
        .find(|index| *index + 1 == input_index)
        .unwrap();
    let high_style = high_lines[composer_top_divider].style;

    assert_eq!(
        line_text(&high_lines[composer_top_divider]),
        "────────────────────"
    );
    assert_eq!(line_text(&high_lines[input_index]), "hello");
    assert_eq!(
        line_text(&high_lines[input_index + 1]),
        "────────────────────"
    );
    assert_eq!(
        off_style,
        Theme::reasoning_input_border(ReasoningLevel::Off)
    );
    assert_eq!(
        high_style,
        Theme::reasoning_input_border(ReasoningLevel::High)
    );
    assert_eq!(high_lines[input_index + 1].style, high_style);
    assert_ne!(off_style, high_style);
}

#[test]
fn active_lines_for_height_uses_actual_viewport_height() {
    let mut app = test_app();
    app.begin_provider_turn_ui();

    let small_lines = app.active_lines_for_height(40, 4);
    let default_lines = app.active_lines_for_height(40, DEFAULT_TUI_HEIGHT as usize);
    let small_rendered = small_lines.iter().map(line_text).collect::<Vec<_>>().join(
        "
",
    );
    let default_rendered = default_lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join(
            "
",
        );

    assert!(!small_rendered.contains("starting"), "{small_rendered}");
    assert!(default_rendered.contains("starting"), "{default_rendered}");
}

#[test]
fn spinner_is_anchored_immediately_above_composer_divider() {
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.tool_calls
        .preview(0, None, vec!["bash".into(), "cargo test".into()]);
    let width = 40;
    let height = 24;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let layout = app.screen_layout(Rect::new(0, 0, width, height), Instant::now());
    let rows = (0..height)
        .map(|row| buffer_row_text(terminal.backend().buffer(), row))
        .collect::<Vec<_>>();
    let activity = layout.activity.unwrap();
    assert_eq!(activity.y.saturating_add(1), layout.top_divider.y);
    assert_eq!(activity.y, layout.history.bottom().saturating_sub(1));
    assert!(activity.width < layout.history.width);
    assert!(rows[activity.y as usize].contains("starting"), "{rows:#?}");
    assert!(
        rows[..activity.y as usize]
            .iter()
            .any(|row| row.contains("cargo test")),
        "{rows:#?}"
    );
}

#[test]
fn active_lines_hide_spinner_when_idle() {
    let mut app = test_app();
    let rendered = app
        .active_lines_at_for_height(40, DEFAULT_TUI_HEIGHT as usize, Instant::now())
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains("starting"), "{rendered}");
}

#[test]
fn draw_anchors_last_live_line_to_viewport_bottom() {
    let mut app = test_app();
    let height = 24;
    let backend = TestBackend::new(60, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let bottom = buffer_row_text(terminal.backend().buffer(), height.saturating_sub(1));
    assert!(bottom.contains("low"), "{bottom:?}");
    assert!(!bottom.contains("ready"), "{bottom:?}");
}

#[test]
fn long_input_keeps_statusline_and_cursor_visible() {
    let mut app = test_app();
    app.input = (0..30)
        .map(|index| format!("line {index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.input_cursor = app.input_char_len();
    let height = 8;
    let backend = TestBackend::new(40, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let rows = (0..height)
        .map(|row| buffer_row_text(terminal.backend().buffer(), row))
        .collect::<Vec<_>>();
    let bottom = rows.last().unwrap();
    let cursor = terminal.backend().cursor_position();
    assert!(rows.iter().any(|row| row.contains("line 29")), "{rows:#?}");
    assert!(bottom.contains("low"), "{bottom:?}");
    assert!(!bottom.contains("ready"), "{bottom:?}");
    assert!(cursor.y < height, "{cursor:?}");
    assert!(
        rows[cursor.y as usize].contains("line 29"),
        "{rows:#?} {cursor:?}"
    );
}

#[test]
fn command_palette_anchors_last_suggestion_to_viewport_bottom() {
    let mut app = test_app();
    app.input = "/m".into();
    app.input_cursor = 2;
    app.clamp_command_selection();
    let height = 24;
    let backend = TestBackend::new(60, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let bottom = buffer_row_text(terminal.backend().buffer(), height.saturating_sub(1));
    assert!(
        bottom.contains("/model") || bottom.contains("/"),
        "{bottom:?}"
    );
    assert!(!bottom.trim().is_empty(), "{bottom:?}");
}

#[test]
fn long_picker_filter_does_not_clip_bottom_status() {
    let mut app = test_app();
    let mut picker = UiPicker::new(
        "models",
        "enter select",
        vec![PickerItem {
            section: None,
            label: "gpt-5.5".into(),
            detail: None,
            preview: None,
            badge: None,
            value: "gpt-5.5".into(),
        }],
        PickerAction::SelectModel,
    );
    picker.filter = "x".repeat(120);
    app.composer = ComposerMode::Picker(picker);
    let height = 24;
    let backend = TestBackend::new(40, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let bottom = buffer_row_text(terminal.backend().buffer(), height.saturating_sub(1));
    assert!(bottom.contains("low"), "{bottom:?}");
    assert!(!bottom.contains("ready"), "{bottom:?}");
}

#[test]
fn command_palette_visibility_tracks_leading_command_token() {
    let mut app = test_app();

    app.input = "/".into();
    app.input_cursor = 1;
    app.clamp_command_selection();
    assert!(app.command_palette_visible());

    app.input = "/mo".into();
    app.input_cursor = 3;
    app.clamp_command_selection();
    assert!(app.command_palette_visible());

    app.input = "/model arg".into();
    app.input_cursor = app.input_char_len();
    app.clamp_command_selection();
    assert!(!app.command_palette_visible());

    app.input = "hello /model".into();
    app.input_cursor = app.input_char_len();
    app.clamp_command_selection();
    assert!(!app.command_palette_visible());
}

#[test]
fn file_palette_stays_inline_with_input_and_inserts_selected_path() {
    use std::fs;

    use tempfile::tempdir;

    file_picker::clear_workspace_file_path_cache();
    let workspace = tempdir().unwrap();
    fs::create_dir_all(workspace.path().join("src")).unwrap();
    fs::write(workspace.path().join("src/lib.rs"), "").unwrap();
    fs::write(workspace.path().join("README.md"), "").unwrap();

    let mut app = test_app();
    app.info.runtime.cwd = workspace.path().to_path_buf();
    app.input = "review @slr".into();
    app.input_cursor = app.input_char_len();
    app.clamp_file_selection();

    assert!(app.file_palette_visible());
    assert!(!matches!(app.composer, ComposerMode::Picker(_)));
    assert_eq!(app.selected_file_path().as_deref(), Some("src/lib.rs"));

    let rendered = app
        .active_lines(60)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("> @src/lib.rs"), "{rendered}");
    assert!(rendered.contains("review @slr"), "{rendered}");

    app.insert_selected_file_path("src/lib.rs");
    assert_eq!(app.input, "review @src/lib.rs ");
    assert!(!app.file_palette_visible());

    app.input = "review @src/lib.rs later".into();
    app.input_cursor = 11;
    app.input_changed();
    app.insert_selected_file_path("src/main.rs");
    assert_eq!(app.input, "review @src/main.rs later");
}

#[test]
fn file_palette_arrow_keys_scroll_beyond_visible_window() {
    use std::fs;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::tempdir;

    file_picker::clear_workspace_file_path_cache();
    let workspace = tempdir().unwrap();
    for index in 0..8 {
        fs::write(workspace.path().join(format!("file-{index}.txt")), "").unwrap();
    }

    let mut app = test_app();
    app.info.runtime.cwd = workspace.path().to_path_buf();
    app.input = "@".into();
    app.input_cursor = 1;
    app.clamp_file_selection();

    let matches = app.file_matches();
    assert!(matches.len() > MAX_COMMAND_SUGGESTIONS, "{matches:?}");

    let top_rendered = app
        .command_suggestion_lines(60)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        top_rendered.contains("↓ 3 more · 8 total"),
        "{top_rendered}"
    );
    assert!(!top_rendered.contains("↑"), "{top_rendered}");

    for _ in 0..MAX_COMMAND_SUGGESTIONS {
        app.handle_file_palette_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
    }

    assert_eq!(app.file_selection, MAX_COMMAND_SUGGESTIONS);
    assert_eq!(
        app.selected_file_path().as_deref(),
        Some(matches[MAX_COMMAND_SUGGESTIONS].as_str())
    );

    let rendered = app
        .command_suggestion_lines(60)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains(&format!("> @{}", matches[MAX_COMMAND_SUGGESTIONS])),
        "{rendered}"
    );
    assert!(
        !rendered.contains(&format!("@{}", matches[0])),
        "expected window to scroll past first match: {rendered}"
    );
    assert!(
        rendered.contains("↑ 1 more · ↓ 2 more · 8 total"),
        "{rendered}"
    );
}

#[test]
fn command_palette_rendering_shows_selected_match() {
    let mut app = test_app();
    app.input = "/m".into();
    app.input_cursor = 2;
    app.clamp_command_selection();

    let rendered = app
        .active_lines(60)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("> /model [model]"), "{rendered}");
    assert!(rendered.contains("show or switch model"), "{rendered}");
}

#[test]
fn command_palette_renders_under_message_box() {
    let mut app = test_app();
    app.input = "/m".into();
    app.input_cursor = 2;
    app.clamp_command_selection();

    let lines = app
        .active_lines(60)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    let input_index = lines
        .iter()
        .position(|line| line.trim_end() == "/m")
        .unwrap();
    let suggestion_index = lines
        .iter()
        .position(|line| line.contains("> /model [model]"))
        .unwrap();

    assert!(suggestion_index > input_index, "{lines:#?}");
}

#[test]
fn picker_renders_in_place_of_message_box() {
    let mut app = test_app();
    app.input = "draft prompt".into();
    app.input_cursor = app.input_char_len();
    app.composer = ComposerMode::Picker(UiPicker::new(
        "select model",
        "enter confirm",
        vec![
            PickerItem {
                section: None,
                label: "model-a".into(),
                detail: None,
                preview: None,
                badge: Some(PickerBadge {
                    text: "(selected)".into(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: "model-a".into(),
            },
            PickerItem {
                section: None,
                label: "model-b".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "model-b".into(),
            },
        ],
        PickerAction::SelectModel,
    ));

    let rendered = app
        .active_lines(60)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("select model"), "{rendered}");
    assert!(rendered.contains("→ model-a"), "{rendered}");
    assert!(!rendered.contains("draft prompt"), "{rendered}");
}

#[test]
fn secret_input_masks_api_key() {
    let mut app = test_app();
    let target = catalog::login_target_for_provider("openai").unwrap();
    let mut secret = SecretInput::new(target);
    secret.insert_text("sk-secret-value");
    app.composer = ComposerMode::SecretInput(secret);

    let rendered = app
        .active_lines(60)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("enter OpenAI API key"), "{rendered}");
    assert!(rendered.contains("••••"), "{rendered}");
    assert!(!rendered.contains("sk-secret-value"), "{rendered}");
}

#[test]
fn login_provider_picker_uses_readable_group_prompts() {
    let labels = catalog::login_groups()
        .into_iter()
        .map(|group| group.prompt.to_string())
        .collect::<Vec<_>>();
    for prompt in [
        "OpenAI",
        "Anthropic",
        "Google Gemini",
        "GitHub Copilot",
        "Moonshot AI",
        "xAI",
    ] {
        assert!(
            labels.iter().any(|label| label == prompt),
            "missing {prompt} in {labels:?}"
        );
    }

    let mut app = test_app();
    app.open_login_picker();
    let rendered = app
        .active_lines(80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    for internal_name in ["openai-codex", "kimi-code", "xai-oauth", "api-key"] {
        assert!(!rendered.contains(internal_name), "{rendered}");
    }
}

#[test]
fn login_method_picker_uses_readable_auth_prompts() {
    let picker = provider_picker::login_method_picker(catalog::login_group("xai").unwrap());
    let labels = picker
        .items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();

    assert_eq!(labels, vec!["API Key", "OAuth"]);
}

#[test]
fn logout_provider_picker_uses_only_providers_with_stored_credentials() {
    let store = MemoryCredentialStore::default();
    save_provider_api_key(&store, "openai", "sk-test").unwrap();
    save_provider_api_key(&store, "anthropic", "sk-ant-test").unwrap();

    let picker = provider_picker::logout_provider_picker(&store).unwrap();
    let values = picker
        .items
        .iter()
        .map(|item| item.value.as_str())
        .collect::<Vec<_>>();

    assert_eq!(values, vec!["anthropic", "openai"]);
}

#[test]
fn logout_provider_picker_propagates_credential_store_errors() {
    let error = provider_picker::logout_provider_picker(&FailingCredentialStore).unwrap_err();

    assert_eq!(
        error.to_string(),
        CredentialError::StoreUnavailable("test failure".into()).to_string()
    );
}

#[test]
fn model_picker_uses_all_available_auths() {
    let store = Arc::new(MemoryCredentialStore::default());
    save_provider_api_key(store.as_ref(), "openai", "sk-test").unwrap();
    save_codex_tokens(
        store.as_ref(),
        &rho_providers::credentials::CodexTokens {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            account_id: None,
        },
    )
    .unwrap();
    save_provider_api_key(store.as_ref(), "anthropic", "sk-ant-test").unwrap();
    let mut app = App::new_with_credentials(
        test_bootstrap(),
        store,
        crate::herdr::HerdrGraphicsCapability::NotHerdr,
    );
    app.refresh_available_auths();

    let models = catalog::available_models_for_auths(&app.available_auths);

    assert!(app.available_auths.iter().any(|auth| auth == "api-key"));
    assert!(app
        .available_auths
        .iter()
        .any(|auth| auth == "anthropic-api-key"));
    assert!(models.iter().any(|model| model.provider == "openai-codex"));
}

#[test]
fn model_picker_fuzzy_matches_and_autocompletes() {
    let mut picker = UiPicker::new(
        "select model",
        "enter confirm",
        vec![
            PickerItem {
                section: None,
                label: "openai/gpt-5.5".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "openai/gpt-5.5".into(),
            },
            PickerItem {
                section: None,
                label: "openai-codex/gpt-5.4-mini".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "openai-codex/gpt-5.4-mini".into(),
            },
        ],
        PickerAction::SelectModel,
    );

    for ch in "ocg54m".chars() {
        picker.push_filter_char(ch);
    }

    assert_eq!(picker.matching_indices(), vec![1]);
    assert_eq!(
        picker.selected_item().unwrap().value,
        "openai-codex/gpt-5.4-mini"
    );
    picker.complete_filter();
    assert_eq!(picker.filter, "openai-codex/gpt-5.4-mini");
}

#[test]
fn picker_lines_render_name_detail_table_with_truncated_detail() {
    let picker = UiPicker::new(
        "loaded skills",
        "enter inserts command",
        vec![PickerItem {
            section: None,
            label: "test-skill".into(),
            detail: Some("this detail is much too long for the available width".into()),
            preview: None,
            badge: None,
            value: "test-skill".into(),
        }],
        PickerAction::InsertSkillCommand,
    );

    let lines = picker_lines(&picker, 36);

    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(!rendered.contains("| detail"), "{rendered}");
    assert!(rendered.contains("→ test-skill"), "{rendered}");
    assert!(
        rendered.contains("this detail is much too long"),
        "{rendered}"
    );
    assert!(rendered.contains("loaded skills"), "{rendered}");
}

#[test]
fn picker_lines_use_single_column_without_details() {
    let picker = UiPicker::new(
        "select model",
        "enter confirm",
        vec![PickerItem {
            section: None,
            label: "openai-codex/gpt-5.3-codex-max".into(),
            detail: None,
            preview: None,
            badge: Some(PickerBadge {
                text: "(selected)".into(),
                tone: PickerBadgeTone::Selected,
            }),
            value: "openai-codex/gpt-5.3-codex-max".into(),
        }],
        PickerAction::SelectModel,
    );

    let lines = picker_lines(&picker, 60);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

    assert!(!rendered.contains("| detail"), "{rendered}");
    assert!(
        rendered.contains("→ openai-codex/gpt-5.3-codex-max"),
        "{rendered}"
    );
    assert!(rendered.contains("(selected)"), "{rendered}");
    assert_eq!(lines[2].spans[2].style.fg, Some(Color::Yellow));
}

#[test]
fn picker_selection_wraps() {
    let mut picker = UiPicker::new(
        "select model",
        "enter confirm",
        vec![
            PickerItem {
                section: None,
                label: "model-a".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "model-a".into(),
            },
            PickerItem {
                section: None,
                label: "model-b".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "model-b".into(),
            },
        ],
        PickerAction::SelectModel,
    );

    picker.select_previous();
    assert_eq!(picker.selected_item().unwrap().value, "model-b");
    picker.select_next();
    assert_eq!(picker.selected_item().unwrap().value, "model-a");
}

#[test]
fn favorite_save_failure_keeps_model_picker_open() {
    let config_dir = tempfile::tempdir().unwrap();
    let mut app = test_app();
    app.info.services.config_repository =
        ConfigRepository::new(Some(config_dir.path().to_path_buf()));
    let selected_value = "openai/gpt-5.5";
    app.composer = ComposerMode::Picker(UiPicker::new(
        "select model",
        "ctrl-p pin/unpin",
        vec![PickerItem {
            section: None,
            label: selected_value.into(),
            detail: None,
            preview: None,
            badge: None,
            value: selected_value.into(),
        }],
        PickerAction::SelectModel,
    ));
    app.toggle_selected_model_favorite().unwrap();

    assert!(matches!(app.composer, ComposerMode::Picker(_)));
    assert_eq!(app.active_picker_selection().unwrap().1, selected_value);
    assert!(app.info.runtime.favorite_models.is_empty());
    assert_eq!(app.status, "config save failed");
    assert!(matches!(
        app.transcript.last(),
        Some(Entry::Error(message)) if message.starts_with("could not save pinned models: ")
    ));
}

#[test]
fn web_search_config_restore_keeps_api_key_row_selected() {
    let config_dir = tempfile::tempdir().unwrap();
    let mut app = test_app();
    app.info.services.config_repository =
        ConfigRepository::new(Some(config_dir.path().join("config.toml")));
    let config = app.info.services.config_repository.load().unwrap();
    let mut picker =
        config_picker::web_search_config_picker(&config, app.credential_store.as_ref());

    App::restore_picker_position(
        &mut picker,
        config_picker::WEB_SEARCH_EXA_KEY_VALUE,
        String::new(),
    );

    assert_eq!(
        picker.selected_item().unwrap().value,
        config_picker::WEB_SEARCH_EXA_KEY_VALUE
    );
}

#[test]
fn esc_from_nested_web_search_config_returns_to_tools_category() {
    let config_dir = tempfile::tempdir().unwrap();
    let mut app = test_app();
    app.info.services.config_repository =
        ConfigRepository::new(Some(config_dir.path().join("config.toml")));
    let config = app.info.services.config_repository.load().unwrap();
    let mut root = config_picker::config_picker(&app.info.runtime, &config);
    App::restore_picker_position(
        &mut root,
        config_picker::TOOLS_CATEGORY_VALUE,
        String::new(),
    );
    let mut parent = config_picker::category_picker(
        config_picker::TOOLS_CATEGORY_VALUE,
        &app.info.runtime,
        &config,
    )
    .unwrap()
    .with_parent(root);
    App::restore_picker_position(&mut parent, config_picker::WEB_SEARCH_VALUE, "web".into());
    app.composer = ComposerMode::Picker(parent);
    let child = config_picker::web_search_config_picker(&config, app.credential_store.as_ref());
    app.open_child_picker(child);

    app.handle_picker_escape(/*running*/ false).unwrap();

    let ComposerMode::Picker(picker) = &app.composer else {
        panic!("expected picker after nested config escape");
    };
    assert_eq!(
        picker.selected_item().unwrap().value,
        config_picker::WEB_SEARCH_VALUE
    );
    assert_eq!(picker.filter, "web");
    assert_eq!(app.status, picker.title);
}

#[test]
fn esc_from_main_config_still_closes_picker() {
    let config_dir = tempfile::tempdir().unwrap();
    let mut app = test_app();
    app.info.services.config_repository =
        ConfigRepository::new(Some(config_dir.path().join("config.toml")));
    let config = app.info.services.config_repository.load().unwrap();
    app.composer = ComposerMode::Picker(config_picker::config_picker(&app.info.runtime, &config));

    app.handle_picker_escape(/*running*/ false).unwrap();

    assert!(matches!(app.composer, ComposerMode::Input));
    assert_eq!(app.status, "ready");
}

#[test]
fn input_history_recalls_previous_messages_and_restores_draft() {
    let mut app = test_app();
    app.push_input_history("first message");
    app.push_input_history("second message");
    app.input = "draft".into();
    app.input_cursor = app.input_char_len();

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input, "second message");
    assert_eq!(app.input_cursor, "second message".chars().count());

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input, "first message");

    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
    assert_eq!(app.input, "second message");

    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
    assert_eq!(app.input, "draft");
    assert_eq!(app.input_history_cursor, None);
}

#[test]
fn input_history_clears_paste_segments_and_restores_draft_segments() {
    let mut app = test_app();
    app.push_input_history("previous message long enough for marker");
    app.insert_pasted_input_text("alpha\nbeta");

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input, "previous message long enough for marker");
    assert!(app.paste_segments.is_empty());
    assert_eq!(
        app.expanded_input(),
        "previous message long enough for marker"
    );

    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
    assert_eq!(app.input, "[ pasted: 2 lines ]");
    assert_eq!(app.expanded_input(), "alpha\nbeta");
}

#[test]
fn editing_input_exits_history_navigation() {
    let mut app = test_app();
    app.push_input_history("previous");
    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);

    app.insert_input_char('!');

    assert_eq!(app.input, "previous!");
    assert_eq!(app.input_history_cursor, None);
    assert_eq!(app.input_history_draft, None);
}

#[test]
fn command_selection_clamps_to_available_matches() {
    let mut app = test_app();
    app.input = "/".into();
    app.input_cursor = 1;
    app.clamp_command_selection();
    app.command_selection = 99;
    app.clamp_command_selection();
    assert_eq!(app.command_selection, app.command_matches().len() - 1);

    app.input = "/mo".into();
    app.input_cursor = 3;
    app.clamp_command_selection();
    assert_eq!(app.command_selection, 0);
}

#[test]
fn command_suggestions_truncate_long_descriptions() {
    let project = tempfile::tempdir().unwrap();
    let skill_dir = project
        .path()
        .join(".agents/skills/zz-deterministic-truncation-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: zz-deterministic-truncation-skill\ndescription: this description is intentionally long enough to require truncation in a narrow command suggestion row\n---\nbody\n",
        )
        .unwrap();
    let mut app = test_app();
    app.info.runtime.cwd = project.path().to_path_buf();
    app.input = "/zz".into();
    app.input_cursor = 3;
    app.clamp_command_selection();

    let lines = app.command_suggestion_lines(40);

    assert!(lines.iter().any(|line| line_text(line).contains('…')));
    assert!(lines
        .iter()
        .all(|line| line_text(line).chars().count() <= 40));
}

#[test]
fn slash_command_args_preserves_text_after_skill_command() {
    assert_eq!(
        slash_command_args("/skill:rust-review check this diff"),
        "check this diff"
    );
}

#[test]
fn complete_slash_command_inserts_prefixed_skill_command() {
    let (input, cursor) = complete_slash_command("/cav", 4, "skill:caveman");

    assert_eq!(input, "/skill:caveman");
    assert_eq!(cursor, 14);
}

#[test]
fn history_lines_include_header_transcript_pending_preview_but_not_activity_row() {
    let mut app = test_app();
    app.push_transcript_entry(Entry::User("hello".into()));
    app.tool_calls
        .preview(0, None, vec!["bash".into(), "cargo test".into()]);
    app.streams.live_stream_preview = Some(LiveStreamPreview {
        kind: StreamKind::Assistant,
        text: "partial answer".into(),
        include_leading_blank: true,
    });
    app.begin_provider_turn_ui();
    app.loading_spinner.start();

    let rendered = app
        .history_lines(60, Instant::now())
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("rho  v"), "{rendered}");
    assert!(rendered.contains("hello"), "{rendered}");
    assert!(rendered.contains("bash"), "{rendered}");
    assert!(rendered.contains("partial answer"), "{rendered}");
    assert!(!rendered.contains("starting"), "{rendered}");
}

#[test]
fn exit_summary_is_minimal_and_session_only() {
    let mut app = test_app();
    assert_eq!(app.exit_summary(), None);

    app.info.session.session_id = Some("session-123".into());
    assert_eq!(
        app.exit_summary().as_deref(),
        Some("rho session saved: session-123")
    );
}

#[test]
fn status_notice_suppresses_consecutive_duplicates() {
    let mut app = test_app();
    app.notify_status("input cleared; press ctrl-c again to quit");
    app.notify_status("input cleared; press ctrl-c again to quit");

    assert_eq!(
            app.transcript
                .iter()
                .filter(|entry| matches!(entry, Entry::Notice(text) if text == "input cleared; press ctrl-c again to quit"))
                .count(),
            1
        );
}

#[test]
fn paste_normalization_converts_crlf_and_cr() {
    assert_eq!(normalize_paste("a\r\nb\rc"), "a\nb\nc");
}
