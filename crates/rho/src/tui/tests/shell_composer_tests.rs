use pretty_assertions::assert_eq;

use super::*;

#[test]
fn shell_mode_labels_top_divider_hides_prefix_without_extra_command_row() {
    let mut app = test_app();
    let empty_height = app.history_height_for_screen(80, 24, Instant::now());

    assert!(app.try_enter_shell_mode_from_bang());
    assert_eq!(
        app.input_ui.shell_mode,
        Some(InlineShellMode::IncludeInContext)
    );
    app.insert_input_text("echo hi");

    assert!(app.command_suggestion_lines(80).is_empty());
    assert_eq!(
        app.history_height_for_screen(80, 24, Instant::now()),
        empty_height,
        "shell mode should not steal history height for a hint row"
    );

    let lines = app.active_lines_for_height(80, 24);
    let texts = lines.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        texts
            .iter()
            .any(|line| line.contains("shell · included in context")),
        "top divider should carry the included-context shell label: {texts:?}"
    );
    assert!(
        texts.iter().any(|line| line.contains("echo hi")),
        "composer should show the shell command: {texts:?}"
    );
    assert!(
        texts.iter().all(|line| !line.contains("!echo")),
        "composer should hide the ! prefix: {texts:?}"
    );

    // Second bang upgrades to local shell mode without leaving a prefix in the buffer.
    app.input_ui.text.clear();
    app.input_ui.cursor = 0;
    assert!(app.try_enter_shell_mode_from_bang());
    assert_eq!(
        app.input_ui.shell_mode,
        Some(InlineShellMode::ExcludeFromContext)
    );
    app.insert_input_text("echo hi");
    let local_lines = app.active_lines_for_height(80, 24);
    let local_texts = local_lines.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        local_texts
            .iter()
            .any(|line| line.contains("shell · excluded from context")),
        "top divider should carry the excluded-context shell label: {local_texts:?}"
    );
    assert!(
        local_texts.iter().all(|line| !line.contains("!!")),
        "composer should hide the !! prefix: {local_texts:?}"
    );
}

#[test]
fn shell_mode_stays_visible_in_narrow_layouts() {
    let mut app = test_app();
    assert!(app.try_enter_shell_mode_from_bang());
    app.insert_input_text("pwd");

    let lines = app.active_lines_for_height(20, 12);
    let texts = lines.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        texts.iter().any(|line| line.contains("shell")),
        "narrow layout should still show a shell mode label: {texts:?}"
    );
    assert!(
        texts.iter().any(|line| line.contains("pwd")),
        "narrow layout should keep the command body: {texts:?}"
    );
}

#[test]
fn escape_exits_shell_mode_and_keeps_command_text() {
    let mut app = test_app();
    assert!(app.try_enter_shell_mode_from_bang());
    app.insert_input_text("echo hi");
    app.input_ui.cursor = app.input_ui.text.chars().count();

    assert!(app.exit_shell_mode());
    assert_eq!(app.input_ui.shell_mode, None);
    assert_eq!(app.input_ui.text, "echo hi");
    assert_eq!(app.input_ui.cursor, "echo hi".chars().count());

    // Re-enter local shell mode after clearing back to an empty composer.
    app.input_ui.text.clear();
    app.input_ui.cursor = 0;
    assert!(app.try_enter_shell_mode_from_bang());
    assert!(app.try_enter_shell_mode_from_bang());
    app.insert_input_text("printf ok");
    app.input_ui.cursor = 2; // inside the visible command
    assert!(app.exit_shell_mode());
    assert_eq!(app.input_ui.text, "printf ok");
    assert_eq!(app.input_ui.cursor, 2);
    assert!(!app.exit_shell_mode());
}

#[test]
fn shell_mode_home_left_right_delete_backspace_word_and_paste_are_coherent() {
    let mut app = test_app();
    assert!(app.try_enter_shell_mode_from_bang());
    app.insert_input_text("echo hello world");

    // Home and left stay on the command body, not a hidden prefix.
    app.input_ui.cursor = app.input_char_len();
    app.input_ui.cursor = 0;
    assert_eq!(app.input_ui.cursor, 0);
    app.move_input_cursor_right();
    assert_eq!(app.input_ui.cursor, 1);
    app.move_input_cursor_left();
    assert_eq!(app.input_ui.cursor, 0);

    // Delete/backspace edit only the command text and leave shell mode intact.
    app.input_ui.cursor = 5; // after "echo "
    app.delete_input();
    assert_eq!(app.input_ui.text, "echo ello world");
    assert_eq!(
        app.input_ui.shell_mode,
        Some(InlineShellMode::IncludeInContext)
    );
    app.backspace_input();
    assert_eq!(app.input_ui.text, "echoello world");
    assert_eq!(
        app.input_ui.shell_mode,
        Some(InlineShellMode::IncludeInContext)
    );

    // Word motion and paste operate in ordinary composer coordinates.
    app.input_ui.text = "one two".into();
    app.input_ui.cursor = app.input_char_len();
    app.input_ui.cursor = previous_word_boundary(&app.input_ui.text, app.input_ui.cursor);
    assert_eq!(app.input_ui.cursor, 4);
    app.insert_input_text("pasted ");
    assert_eq!(app.input_ui.text, "one pasted two");
    assert_eq!(
        app.input_ui.shell_mode,
        Some(InlineShellMode::IncludeInContext)
    );
}

#[test]
fn bang_key_enters_and_upgrades_shell_mode_without_buffer_prefix() {
    let mut app = test_app();
    app.insert_input_char('!');
    assert_eq!(
        app.input_ui.shell_mode,
        Some(InlineShellMode::IncludeInContext)
    );
    assert_eq!(app.input_ui.text, "");
    assert_eq!(app.input_ui.cursor, 0);

    app.insert_input_char('!');
    assert_eq!(
        app.input_ui.shell_mode,
        Some(InlineShellMode::ExcludeFromContext)
    );
    assert_eq!(app.input_ui.text, "");

    app.insert_input_char('l');
    assert_eq!(app.input_ui.text, "l");
    assert_eq!(
        app.input_ui.shell_mode,
        Some(InlineShellMode::ExcludeFromContext)
    );
}

#[test]
fn slash_prefixed_shell_commands_do_not_open_the_command_palette() {
    let mut app = test_app();
    app.insert_input_char('!');
    app.insert_input_text("/help");

    assert!(!app.command_palette_visible());
    assert_eq!(
        app.shell_submission(),
        Some((InlineShellMode::IncludeInContext, "/help".into()))
    );

    app.begin_provider_turn_ui();
    assert!(!app.command_palette_visible());
    assert_eq!(
        app.shell_submission(),
        Some((InlineShellMode::IncludeInContext, "/help".into()))
    );
}

#[test]
fn history_recall_restores_shell_mode_from_prefixed_entries() {
    let mut app = test_app();
    app.push_input_history("!echo hi");
    app.push_input_history("!!ls -la");

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(
        app.input_ui.shell_mode,
        Some(InlineShellMode::ExcludeFromContext)
    );
    assert_eq!(app.input_ui.text, "ls -la");

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(
        app.input_ui.shell_mode,
        Some(InlineShellMode::IncludeInContext)
    );
    assert_eq!(app.input_ui.text, "echo hi");
}

#[test]
fn dismiss_overlay_accepts_spaces_in_regex_filters() {
    let mut picker = help_picker::help_picker(&crate::keybindings::Keybindings::default());
    assert_eq!(picker.action, PickerAction::Dismiss);
    assert!(!picker.action.space_confirms_selection());
    assert!(picker.action.uses_regex_filter());

    for ch in "shell command".chars() {
        picker.push_filter_char(ch);
    }
    assert_eq!(picker.filter, "shell command");
    assert!(
        !picker.matching_indices().is_empty(),
        "regex filter with spaces should keep matching help entries"
    );

    // Space must type into the filter rather than confirming dismiss.
    assert!(!picker.action.space_confirms_selection());
    picker.push_filter_char(' ');
    assert_eq!(picker.filter, "shell command ");
}

#[test]
fn labeled_divider_falls_back_to_shortest_shell_label() {
    let line = labeled_divider_line(
        inline_shell::mode_divider_labels(InlineShellMode::IncludeInContext),
        Theme::dim(),
        12,
    )
    .expect("shortest label should fit");
    let text = line_text(&line);
    assert!(text.contains("shell"), "{text}");
    assert!(!text.contains("included"), "{text}");
}
