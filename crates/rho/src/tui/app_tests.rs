use super::{
    command_palette::{complete_slash_command, slash_command_args},
    message_history::{recovered_history_tail, transcript_entries_from_messages},
    paste_burst::normalize_paste,
    tool_output_ui::expandable_tool_entry,
    transcript_events::final_answer_delta,
    *,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;
use rho_providers::credentials::{
    save_provider_api_key, CredentialError, CredentialResult, MemoryCredentialStore,
};
use std::sync::atomic::AtomicBool;

#[path = "tests/activity_phase_tests.rs"]
mod activity_phase_tests;
#[path = "tests/input_editing_tests.rs"]
mod input_editing_tests;
#[path = "tests/questionnaire_interaction_tests.rs"]
mod questionnaire_interaction_tests;
#[path = "tests/reasoning_chrome_tests.rs"]
mod reasoning_chrome_tests;
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
            service_tier: None,
            reasoning_source: ReasoningRequestSource::PersistedOrDefault,
            permission_mode: PermissionMode::Auto,
            show_reasoning_output: true,
            zen_mode: false,
            advisor_mode: false,
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
            theme: "terminal".into(),
            first_run: None,
            auth_unavailable: None,
            update_notice: None,
            pending_update_notice: None,
            pending_custom_models: None,
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

fn test_tool_entry(ok: bool, display_lines: &[&str]) -> Entry {
    let status = if ok {
        rho_tools::tool_card::ToolStatus::Ok
    } else {
        rho_tools::tool_card::ToolStatus::Error
    };
    let heading = display_lines.first().copied().unwrap_or("tool");
    let mut card = rho_tools::tool_card::ToolCard::new(
        status,
        rho_tools::tool_card::ToolFamily::Default,
        rho_tools::tool_card::ToolHeader::call(heading, None),
    );
    if display_lines.len() > 1 {
        card = card.with_body(rho_tools::tool_card::ToolBody::Lines(
            display_lines[1..]
                .iter()
                .map(|line| (*line).to_string())
                .collect(),
        ));
    }
    Entry::Tool(ToolEntry {
        card,
        expanded: false,
        image: None,
        started_at: None,
    })
}

pub(super) fn test_app() -> App {
    let store = Arc::new(MemoryCredentialStore::default());
    save_provider_api_key(store.as_ref(), "openai", "sk-test").unwrap();
    App::new_with_credentials(
        test_bootstrap(),
        store,
        crate::herdr::HerdrGraphicsCapability::NotHerdr,
        crate::tools::mcp::McpSessionReport::default(),
        crate::tools::mcp::McpCatalog::default(),
        crate::plugins::PluginLoadReport::default(),
    )
}

fn paste_lines(n: usize) -> String {
    (1..=n)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn collapsible_paste() -> String {
    paste_lines(super::paste_burst::PASTE_COLLAPSE_MIN_LINES)
}

fn collapsed_paste_marker() -> String {
    format!(
        "[ pasted: {} lines ]",
        super::paste_burst::PASTE_COLLAPSE_MIN_LINES
    )
}

// Covers: at the minimum usable size the composer keeps a visible row.
// Owner: pure layout
#[test]
fn minimum_terminal_layout_keeps_composer_visible() {
    use ratatui::layout::Rect;

    let mut app = test_app();
    let area = Rect::new(
        0,
        0,
        super::screen_layout::MIN_TERMINAL_WIDTH,
        super::screen_layout::MIN_TERMINAL_HEIGHT,
    );
    let width = area.width as usize;
    let composer_lines = app.composer_lines(width, area.height as usize);
    assert!(!composer_lines.is_empty());
    let layout = app.screen_layout_for_history_len(
        area,
        /*history_len*/ 0,
        &composer_lines,
        /*command_line_count*/ 0,
    );
    assert!(
        layout.composer.height >= 1,
        "composer height at minimum size was {}",
        layout.composer.height
    );
    assert_eq!(layout.statusline.height, 2);
    assert_eq!(layout.bottom_divider.height, 1);
}

#[test]
fn interrupt_during_tool_ends_turn_immediately() {
    let mut app = test_app();
    let interrupt_requested = AtomicBool::new(false);
    let tool_call_active = AtomicBool::new(true);

    let control = app.request_running_interrupt(&interrupt_requested, &tool_call_active);

    assert!(interrupt_requested.load(Ordering::SeqCst));
    assert!(matches!(control, StreamControl::Interrupt));
    assert_eq!(app.status(), "interrupting tool");
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
fn recovered_history_tail_limits_initial_redraw() {
    let entries = (0..10)
        .map(|index| Entry::User(format!("message {index}")))
        .collect::<Vec<_>>();

    let (omitted, visible) = recovered_history_tail(
        &entries,
        9,
        crate::tui::history_cache::HistoryRenderSettings {
            width: 80,
            max_tool_output_lines: 10,
            zen_mode: false,
            theme_generation: 0,
            max_image_height: crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT,
        },
    );

    // Each user entry is one content line + trailing blank (2 rows). A 9-line
    // budget therefore keeps four tail messages.
    assert_eq!(omitted, 6);
    assert!(matches!(visible.as_slice(), [
            Entry::User(a),
            Entry::User(b),
            Entry::User(c),
            Entry::User(d),
        ] if a == "message 6" && b == "message 7" && c == "message 8" && d == "message 9"));
}

// Covers: zen-hidden suffix must not drop an oversized latest visible entry.
// Owner: pure recovery selection policy.
#[test]
fn recovered_history_tail_keeps_oversized_visible_before_hidden_suffix() {
    use crate::tui::{ReasoningEntry, ToolEntry};

    let oversized = Entry::Assistant("line\n".repeat(40));
    let tool = Entry::Tool(ToolEntry {
        card: rho_tools::tool_card::ToolCard::new(
            rho_tools::tool_card::ToolStatus::Running,
            rho_tools::tool_card::ToolFamily::Default,
            rho_tools::tool_card::ToolHeader::call("read_file(a.rs)", None),
        ),
        expanded: false,
        image: None,
        started_at: None,
    });
    let entries = vec![
        Entry::User("earlier".into()),
        oversized,
        tool,
        Entry::Reasoning(ReasoningEntry::new("hidden plan")),
    ];

    let (omitted, visible) = recovered_history_tail(
        &entries,
        5,
        crate::tui::history_cache::HistoryRenderSettings {
            width: 40,
            max_tool_output_lines: 10,
            zen_mode: true,
            theme_generation: 0,
            max_image_height: crate::tui::feed_image::DEFAULT_IMAGE_HEIGHT,
        },
    );

    assert_eq!(omitted, 1);
    assert_eq!(visible.len(), 3);
    assert!(matches!(visible[0], Entry::Assistant(_)));
    assert!(matches!(visible[1], Entry::Tool(_)));
    assert!(matches!(visible[2], Entry::Reasoning(_)));
}

#[test]
fn key_event_paste_burst_collapses_through_common_paste_path() {
    let start = Instant::now();
    let mut app = test_app();
    let pasted = collapsible_paste();

    for (index, ch) in pasted.chars().enumerate() {
        let key = if ch == '\n' {
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        } else {
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
        };
        assert!(app.handle_paste_burst_key_at(key, start + Duration::from_millis(index as u64)));
    }
    app.flush_pending_paste_burst();

    assert_eq!(app.input_ui.text(), collapsed_paste_marker());
    assert_eq!(app.expanded_input(), pasted);
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

    assert_eq!(app.input_ui.text(), "a");
    assert!(app.input_ui.paste_segments().is_empty());
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

    assert_eq!(app.input_ui.text(), "");
    assert!(app.input_ui.paste_segments().is_empty());
}

#[test]
fn pasted_multiline_input_collapses_to_marker_and_expands() {
    let mut app = test_app();
    let pasted = collapsible_paste();

    app.insert_pasted_input_text(&pasted);

    assert_eq!(app.input_ui.text(), collapsed_paste_marker());
    assert_eq!(app.input_ui.cursor(), app.input_ui.text().chars().count());
    assert_eq!(app.expanded_input(), pasted);
}

#[test]
fn pasted_short_input_stays_literal_until_collapse_threshold() {
    let mut app = test_app();

    app.insert_pasted_input_text("hello world");

    assert_eq!(app.input_ui.text(), "hello world");
    assert!(app.input_ui.paste_segments().is_empty());
    assert_eq!(app.expanded_input(), "hello world");

    app.input_ui.clear_text();
    app.input_ui.clear_paste_segments();
    let short_multiline = paste_lines(super::paste_burst::PASTE_COLLAPSE_MIN_LINES - 1);
    app.insert_pasted_input_text(&short_multiline);

    assert_eq!(app.input_ui.text(), short_multiline);
    assert!(app.input_ui.paste_segments().is_empty());
    assert_eq!(app.expanded_input(), short_multiline);
}

#[test]
fn paste_segments_shift_after_edits_before_marker() {
    let mut app = test_app();
    let pasted = collapsible_paste();
    app.insert_pasted_input_text(&pasted);
    app.input_ui.set_cursor(0);
    app.insert_input_text("prefix ");

    assert_eq!(
        app.input_ui.text(),
        format!("prefix {}", collapsed_paste_marker())
    );
    assert_eq!(app.expanded_input(), format!("prefix {pasted}"));
}

#[test]
fn queued_pasted_prompt_keeps_marker_when_recalled_for_editing() {
    let mut app = test_app();
    let pasted = collapsible_paste();
    app.insert_pasted_input_text(&pasted);
    let queued = QueuedPrompt {
        prompt: app.expanded_input(),
        display_prompt: app.input_ui.text().to_string(),
        paste_segments: app.input_ui.paste_segments().to_vec(),
        media: Vec::new(),
    };
    app.input_ui.clear_text();
    app.input_ui.clear_paste_segments();
    app.pending.push_follow_up(queued);

    assert!(app.handle_pending_input_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT,)));
    assert_eq!(app.input_ui.text(), collapsed_paste_marker());
    assert_eq!(app.expanded_input(), pasted);
}

#[test]
fn queued_pasted_prompt_preserves_leading_space_segment_offsets() {
    let mut app = test_app();
    let pasted = collapsible_paste();
    app.insert_input_text(" ");
    app.insert_pasted_input_text(&pasted);
    let queued = QueuedPrompt {
        prompt: app.expanded_input().trim().to_string(),
        display_prompt: app.input_ui.text().to_string(),
        paste_segments: app.input_ui.paste_segments().to_vec(),
        media: Vec::new(),
    };
    app.input_ui.clear_text();
    app.input_ui.clear_paste_segments();
    app.pending.push_follow_up(queued);

    assert!(app.handle_pending_input_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT,)));
    assert_eq!(
        app.input_ui.text(),
        format!(" {}", collapsed_paste_marker())
    );
    assert_eq!(app.expanded_input().trim(), pasted);
}

#[test]
fn slash_command_args_can_keep_collapsed_display_separate_from_expanded_prompt() {
    let mut app = test_app();
    let pasted = collapsible_paste();
    app.insert_input_text("/skill:test ");
    app.insert_pasted_input_text(&pasted);

    let expanded_input = app.expanded_input();
    assert_eq!(slash_command_args(&expanded_input).trim(), pasted);
    assert_eq!(
        slash_command_args(app.input_ui.text()).trim(),
        collapsed_paste_marker()
    );
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
            ref card,
            ..
        }) if card.header_text().contains("read_file")
            && card.header_text().contains("src/main.rs")
            && card.status == rho_tools::tool_card::ToolStatus::Error
    ));
}

#[test]
fn toggling_latest_truncated_tool_collapses_previous_tool() {
    let mut app = test_app();
    app.info.runtime.max_tool_output_lines = 1;
    app.history.set_entries(vec![
        test_tool_entry(true, &["first", "a\nb"]),
        test_tool_entry(true, &["second", "c\nd"]),
    ]);
    if let Entry::Tool(tool) = &mut app.history.entries_mut()[0] {
        tool.expanded = true;
    }

    let index = app
        .history
        .entries()
        .iter()
        .rposition(|entry| expandable_tool_entry(entry, app.info.runtime.max_tool_output_lines, 80))
        .unwrap();
    for entry in app.history.entries_mut() {
        if let Entry::Tool(tool) = entry {
            tool.expanded = false;
        }
    }
    if let Entry::Tool(tool) = &mut app.history.entries_mut()[index] {
        tool.expanded = true;
    }

    assert!(matches!(
        app.history.entries()[0],
        Entry::Tool(ToolEntry {
            expanded: false,
            ..
        })
    ));
    assert!(matches!(
        app.history.entries()[1],
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
        app.history.entries(),
        [Entry::Assistant(text)] if text == "final"
    ));
}

#[test]
fn final_answer_mismatch_replaces_transcript_with_empty_answer() {
    let mut app = test_app();
    app.push_transcript_entry(Entry::Assistant("streamed".into()));

    app.replace_current_turn_assistant_transcript("");

    assert!(matches!(
        app.history.entries(),
        [Entry::Assistant(text)] if text.is_empty()
    ));
}

#[test]
fn final_answer_mismatch_replaces_interleaved_current_turn_assistant_fragments() {
    let mut app = test_app();
    app.push_transcript_entry(Entry::User("prompt".into()));
    app.turn.set_current_turn_start(Some(app.history.len()));
    app.push_transcript_entry(Entry::Assistant("hel".into()));
    app.push_transcript_entry(Entry::Reasoning("thinking".into()));
    app.push_transcript_entry(Entry::Assistant("lo".into()));

    app.replace_current_turn_assistant_transcript("goodbye");

    assert!(matches!(
        app.history.entries(),
        [Entry::User(_), Entry::Assistant(text), Entry::Reasoning(_)] if text == "goodbye"
    ));
}

#[test]
fn secret_input_masks_api_key() {
    let mut app = test_app();
    let target = catalog::login_target_for_provider("openai").unwrap();
    let mut secret = SecretInput::new(target);
    secret.insert_text("sk-secret-value");
    app.input_ui.set_composer(ComposerMode::SecretInput(secret));

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

/// Points device-key discovery at an empty directory so the developer's real
/// `~/.ollama` key cannot leak into credential-availability assertions.

#[tokio::test]
async fn logout_provider_picker_propagates_credential_store_errors() {
    let error = provider_picker::logout_provider_picker(
        &FailingCredentialStore,
        /*claude_signed_in*/ false,
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        CredentialError::StoreUnavailable("test failure".into()).to_string()
    );
}

#[test]
fn model_picker_fuzzy_matches_and_autocompletes() {
    let mut picker = UiPicker::new(
        "select model",
        vec![
            PickerItem {
                section: None,
                label: "openai/gpt-5.5".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "openai/gpt-5.5".into(),
                selection_verb: None,
            },
            PickerItem {
                section: None,
                label: "openai-codex/gpt-5.4-mini".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "openai-codex/gpt-5.4-mini".into(),
                selection_verb: None,
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
fn picker_selection_wraps() {
    let mut picker = UiPicker::new(
        "select model",
        vec![
            PickerItem {
                section: None,
                label: "model-a".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "model-a".into(),
                selection_verb: None,
            },
            PickerItem {
                section: None,
                label: "model-b".into(),
                detail: None,
                preview: None,
                badge: None,
                value: "model-b".into(),
                selection_verb: None,
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
    app.input_ui
        .set_composer(ComposerMode::Picker(UiPicker::new(
            "select model",
            vec![PickerItem {
                section: None,
                label: selected_value.into(),
                detail: None,
                preview: None,
                badge: None,
                value: selected_value.into(),
                selection_verb: None,
            }],
            PickerAction::SelectModel,
        )));
    app.toggle_selected_model_favorite().unwrap();

    assert!(matches!(app.input_ui.composer(), ComposerMode::Picker(_)));
    assert_eq!(app.active_picker_selection().unwrap().1, selected_value);
    assert!(app.info.runtime.favorite_models.is_empty());
    assert!(matches!(app.history.last(), Some(Entry::Error(_))));
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
    app.input_ui.set_composer(ComposerMode::Picker(parent));
    let child = config_picker::web_search_config_picker(&config, app.credential_store.as_ref());
    app.open_child_picker(child);

    app.handle_picker_escape(/*running*/ false).unwrap();

    let ComposerMode::Picker(picker) = app.input_ui.composer() else {
        panic!("expected picker after nested config escape");
    };
    assert_eq!(
        picker.selected_item().unwrap().value,
        config_picker::WEB_SEARCH_VALUE
    );
    assert_eq!(picker.filter, "web");
}

#[test]
fn esc_from_main_config_still_closes_picker() {
    let config_dir = tempfile::tempdir().unwrap();
    let mut app = test_app();
    app.info.services.config_repository =
        ConfigRepository::new(Some(config_dir.path().join("config.toml")));
    let config = app.info.services.config_repository.load().unwrap();
    app.input_ui
        .set_composer(ComposerMode::Picker(config_picker::config_picker(
            &app.info.runtime,
            &config,
        )));

    app.handle_picker_escape(/*running*/ false).unwrap();

    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
}

#[test]
fn input_history_recalls_previous_messages_and_restores_draft() {
    let mut app = test_app();
    app.push_input_history("first message");
    app.push_input_history("second message");
    app.input_ui.set_text("draft".to_string());
    app.input_ui.set_cursor(app.input_char_len());

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input_ui.text(), "second message");
    assert_eq!(app.input_ui.cursor(), "second message".chars().count());

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input_ui.text(), "first message");

    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
    assert_eq!(app.input_ui.text(), "second message");

    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
    assert_eq!(app.input_ui.text(), "draft");
    assert_eq!(app.input_ui.history_cursor(), None);
}

#[test]
fn input_history_clears_paste_segments_and_restores_draft_segments() {
    let mut app = test_app();
    let pasted = collapsible_paste();
    app.push_input_history("previous message long enough for marker");
    app.insert_pasted_input_text(&pasted);

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(
        app.input_ui.text(),
        "previous message long enough for marker"
    );
    assert!(app.input_ui.paste_segments().is_empty());
    assert_eq!(
        app.expanded_input(),
        "previous message long enough for marker"
    );

    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
    assert_eq!(app.input_ui.text(), collapsed_paste_marker());
    assert_eq!(app.expanded_input(), pasted);
}

#[test]
fn editing_input_exits_history_navigation() {
    let mut app = test_app();
    app.push_input_history("previous");
    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);

    app.insert_input_char('!');

    assert_eq!(app.input_ui.text(), "previous!");
    assert_eq!(app.input_ui.history_cursor(), None);
    assert_eq!(app.input_ui.history_cursor(), None);
}

#[test]
fn command_selection_clamps_to_available_matches() {
    let mut app = test_app();
    app.input_ui.set_text("/".to_string());
    app.input_ui.set_cursor(1);
    app.clamp_command_selection();
    app.input_ui.set_command_selection(99);
    app.clamp_command_selection();
    assert_eq!(
        app.input_ui.command_selection(),
        app.command_matches().len() - 1
    );

    app.input_ui.set_text("/mo".to_string());
    app.input_ui.set_cursor(3);
    app.clamp_command_selection();
    assert_eq!(app.input_ui.command_selection(), 0);
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

// Covers: ephemeral status feedback must not write transcript notices
// Owner: tui status surface
#[test]
fn notify_status_is_toast_only() {
    let mut app = test_app();
    app.notify_status("notice-a");
    app.notify_status("notice-a");

    assert_eq!(app.status(), "notice-a");
    assert!(app
        .history
        .entries()
        .iter()
        .all(|entry| !matches!(entry, Entry::Notice(text) if text == "notice-a")));
}

// Covers: status writes must show as a short-lived overlay toast
// Owner: tui status surface
#[test]
fn status_feedback_sets_visible_overlay() {
    let mut app = test_app();
    app.set_status("interrupting tool");

    let overlay = app
        .status_overlay
        .as_ref()
        .expect("status overlay should be set");
    assert_eq!(overlay.message(), "interrupting tool");
    assert_eq!(overlay.tone(), super::status_overlay::StatusTone::Warning);
    assert!(overlay.is_visible(std::time::Instant::now()));
}

// Covers: silent mode labels must clear a prior toast overlay
// Owner: tui status surface
#[test]
fn silent_status_clears_prior_toast() {
    let mut app = test_app();
    app.set_status("interrupting tool");
    assert!(app.status_overlay.is_some());

    app.set_status("ready");

    assert_eq!(app.status(), "ready");
    assert!(app.status_overlay.is_none());
}
