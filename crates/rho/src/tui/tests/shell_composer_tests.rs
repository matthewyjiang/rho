use pretty_assertions::assert_eq;

use super::*;

#[test]
fn escape_exits_shell_mode_and_keeps_command_text() {
    let mut app = test_app();
    assert!(app.try_enter_shell_mode_from_bang());
    app.insert_input_text("echo hi");
    app.input_ui.set_cursor(app.input_ui.text().chars().count());

    assert!(app.exit_shell_mode());
    assert_eq!(app.input_ui.shell_mode(), None);
    assert_eq!(app.input_ui.text(), "echo hi");
    assert_eq!(app.input_ui.cursor(), "echo hi".chars().count());

    // Re-enter local shell mode after clearing back to an empty composer.
    app.input_ui.clear_text();
    app.input_ui.set_cursor(0);
    assert!(app.try_enter_shell_mode_from_bang());
    assert!(app.try_enter_shell_mode_from_bang());
    app.insert_input_text("printf ok");
    app.input_ui.set_cursor(2); // inside the visible command
    assert!(app.exit_shell_mode());
    assert_eq!(app.input_ui.text(), "printf ok");
    assert_eq!(app.input_ui.cursor(), 2);
    assert!(!app.exit_shell_mode());
}

#[test]
fn shell_mode_home_left_right_delete_backspace_word_and_paste_are_coherent() {
    let mut app = test_app();
    assert!(app.try_enter_shell_mode_from_bang());
    app.insert_input_text("echo hello world");

    // Home and left stay on the command body, not a hidden prefix.
    app.input_ui.set_cursor(app.input_char_len());
    app.input_ui.set_cursor(0);
    assert_eq!(app.input_ui.cursor(), 0);
    app.move_input_cursor_right();
    assert_eq!(app.input_ui.cursor(), 1);
    app.move_input_cursor_left();
    assert_eq!(app.input_ui.cursor(), 0);

    // Delete/backspace edit only the command text and leave shell mode intact.
    app.input_ui.set_cursor(5); // after "echo "
    app.delete_input();
    assert_eq!(app.input_ui.text(), "echo ello world");
    assert_eq!(
        app.input_ui.shell_mode(),
        Some(InlineShellMode::IncludeInContext)
    );
    app.backspace_input();
    assert_eq!(app.input_ui.text(), "echoello world");
    assert_eq!(
        app.input_ui.shell_mode(),
        Some(InlineShellMode::IncludeInContext)
    );

    // Word motion and paste operate in ordinary composer coordinates.
    app.input_ui.set_text("one two".to_string());
    app.input_ui.set_cursor(4); // start of "two"
    assert_eq!(app.input_ui.cursor(), 4);
    app.insert_input_text("pasted ");
    assert_eq!(app.input_ui.text(), "one pasted two");
    assert_eq!(
        app.input_ui.shell_mode(),
        Some(InlineShellMode::IncludeInContext)
    );
}

#[test]
fn bang_key_enters_and_upgrades_shell_mode_without_buffer_prefix() {
    let mut app = test_app();
    app.insert_input_char('!');
    assert_eq!(
        app.input_ui.shell_mode(),
        Some(InlineShellMode::IncludeInContext)
    );
    assert_eq!(app.input_ui.text(), "");
    assert_eq!(app.input_ui.cursor(), 0);

    app.insert_input_char('!');
    assert_eq!(
        app.input_ui.shell_mode(),
        Some(InlineShellMode::ExcludeFromContext)
    );
    assert_eq!(app.input_ui.text(), "");

    app.insert_input_char('l');
    assert_eq!(app.input_ui.text(), "l");
    assert_eq!(
        app.input_ui.shell_mode(),
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
        app.input_ui.shell_mode(),
        Some(InlineShellMode::ExcludeFromContext)
    );
    assert_eq!(app.input_ui.text(), "ls -la");

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(
        app.input_ui.shell_mode(),
        Some(InlineShellMode::IncludeInContext)
    );
    assert_eq!(app.input_ui.text(), "echo hi");
}

#[test]
fn paste_burst_flushed_bang_enters_shell_mode_like_typed_char() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;

    let mut app = test_app();
    let start = Instant::now();
    assert!(
        app.handle_paste_burst_key_at(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE), start)
    );
    // Non-burst key flushes pending text through insert_paste -> insert_input_text.
    assert!(!app.handle_paste_burst_key_at(
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        start + Duration::from_millis(20),
    ));
    assert_eq!(
        app.input_ui.shell_mode(),
        Some(InlineShellMode::IncludeInContext)
    );
    assert_eq!(app.input_ui.text(), "");
    assert_eq!(app.input_ui.cursor(), 0);
}

#[test]
fn paste_burst_flushed_double_bang_enters_local_shell_mode() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;

    let mut app = test_app();
    let start = Instant::now();
    assert!(
        app.handle_paste_burst_key_at(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE), start)
    );
    assert!(app.handle_paste_burst_key_at(
        KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE),
        start + Duration::from_millis(1),
    ));
    assert!(!app.handle_paste_burst_key_at(
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        start + Duration::from_millis(20),
    ));
    assert_eq!(
        app.input_ui.shell_mode(),
        Some(InlineShellMode::ExcludeFromContext)
    );
    assert_eq!(app.input_ui.text(), "");
}

#[test]
fn paste_burst_flushed_bang_command_keeps_body_without_prefix() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::time::Duration;

    let mut app = test_app();
    let start = Instant::now();
    for (i, ch) in ['!', 'e', 'c', 'h', 'o'].into_iter().enumerate() {
        assert!(app.handle_paste_burst_key_at(
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            start + Duration::from_millis(i as u64),
        ));
    }
    assert!(!app.handle_paste_burst_key_at(
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        start + Duration::from_millis(20),
    ));
    assert_eq!(
        app.input_ui.shell_mode(),
        Some(InlineShellMode::IncludeInContext)
    );
    assert_eq!(app.input_ui.text(), "echo");
}
