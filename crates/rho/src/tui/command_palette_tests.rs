use super::super::{tests::test_app, CommandChoice, CommandChoiceKind, InputSubmissionMode};
use crate::tui::composer_buffer::ComposerEditKey;
use pretty_assertions::assert_eq;

#[test]
fn argument_choice_completion_replaces_placeholder() {
    let mut app = test_app();
    app.input_ui.set_text("/codemode ".to_string());
    app.input_ui.set_cursor(app.input_ui.text().chars().count());
    app.clamp_command_selection();
    app.input_ui.move_command_selection(1);

    let on = app.selected_command().unwrap();
    app.complete_command_choice(&on);

    assert_eq!(app.input_ui.text(), "/codemode on");
    assert_eq!(app.input_ui.cursor(), "/codemode on".chars().count());
    assert_eq!(
        app.input_ui.submission_mode(),
        InputSubmissionMode::ParseCommands
    );
}

// Covers: a held prompt resolves a template file as it is on disk now, so
// edits made during a session apply with no restart.
// Owner: prompt template resolution on submit
#[test]
fn prompt_template_resolves_edits_without_restart() {
    let project = tempfile::tempdir().unwrap();
    let prompts = project.path().join(".rho/prompts");
    std::fs::create_dir_all(project.path().join(".git")).unwrap();
    std::fs::create_dir_all(&prompts).unwrap();
    let mut app = test_app();
    app.info.runtime.cwd = project.path().to_path_buf();

    std::fs::write(prompts.join("zz-reload.md"), "first draft").unwrap();
    assert_eq!(
        app.resolve_prompt_template("prompt:zz-reload", ""),
        Some("first draft".to_string())
    );
    std::fs::write(prompts.join("zz-reload.md"), "second draft").unwrap();
    assert_eq!(
        app.resolve_prompt_template("PrOmPt:ZZ-RELOAD", "details"),
        Some("second draft details".to_string())
    );
    assert_eq!(app.resolve_prompt_template("prompt:missing", ""), None);
}

// Covers: recalling a slash command must not steal Up/Down for palette
// matching; typing after recall reopens the list.
// Owner: pure unit (composer history vs palette visibility)
#[test]
fn recalling_a_command_keeps_the_palette_closed_until_edit() {
    let mut app = test_app();
    app.push_input_history("/info");
    app.push_input_history("/model");
    app.input_ui.set_text("/c".to_string());
    app.input_ui.set_cursor(2);
    app.input_changed();
    assert!(app.command_palette_visible());

    app.apply_input_edit_key(ComposerEditKey::Up);
    assert_eq!(app.input_ui.text(), "/model");
    assert!(!app.command_palette_visible());

    app.apply_input_edit_key(ComposerEditKey::Up);
    assert_eq!(app.input_ui.text(), "/info");
    assert!(!app.command_palette_visible());

    app.apply_input_edit_key(ComposerEditKey::Down);
    app.apply_input_edit_key(ComposerEditKey::Down);
    assert_eq!(app.input_ui.text(), "/c");
    assert!(app.command_palette_visible());

    app.apply_input_edit_key(ComposerEditKey::Up);
    assert!(!app.command_palette_visible());
    app.apply_input_edit_key(ComposerEditKey::Backspace);
    assert_eq!(app.input_ui.text(), "/mode");
    assert!(app.command_palette_visible());
}

// Covers: Esc on a bare slash must not leave a trap for blind retyping into
// //cmd, while a partial slash command keeps its typed content.
// Owner: command palette dismiss policy (idle and during-turn Esc arms)
#[test]
fn dismiss_on_esc_clears_only_a_bare_slash() {
    for (typed, expected) in [("/", ""), ("/mo", "/mo")] {
        let mut app = test_app();
        app.input_ui.set_text(typed.to_string());
        app.input_ui.set_cursor(typed.chars().count());
        assert!(app.command_palette_visible(), "{typed}");

        app.dismiss_command_palette_on_esc();

        assert_eq!(app.input_ui.text(), expected, "{typed}");
        assert_eq!(app.input_ui.cursor(), expected.chars().count(), "{typed}");
        assert!(app.input_ui.command_palette_dismissed(), "{typed}");
    }
}

// Covers: with argument rows open, Enter must not spend the default
// highlight on a row nobody picked; an arrow-key pick enables it, and a
// changed question takes the pick back.
// Owner: command palette Enter policy
#[test]
fn enter_keeps_bare_command_until_an_argument_row_is_picked() {
    let mut app = test_app();
    app.input_ui.set_text("/codemode ".to_string());
    app.input_ui.set_cursor(app.input_ui.text().chars().count());
    app.clamp_command_selection();
    let matches = app.command_matches();

    assert!(!app.enter_completes_choice(&matches[0]));

    app.input_ui.move_command_selection(1);
    assert!(app.enter_completes_choice(&matches[1]));

    // Moving the cursor back into the command token changes the palette
    // question, so the pick does not survive it.
    app.input_ui.set_text("/codemode".to_string());
    app.input_ui.set_cursor(app.input_ui.text().chars().count());
    app.input_changed();
    assert!(!app.input_ui.command_selection_explicit());
}

// Covers: when the match list shrinks below the picked row, that row is
// gone, so the clamped highlight must not act as a pick for Enter.
// Owner: command palette selection invariant
#[test]
fn clamping_below_a_pick_drops_the_explicit_flag() {
    let mut app = test_app();
    app.input_ui.set_text("/codemode ".to_string());
    app.input_ui.set_cursor(app.input_ui.text().chars().count());
    app.clamp_command_selection();
    app.input_ui.move_command_selection(1);
    assert!(app.input_ui.command_selection_explicit());

    // `/agents ` offers a single argument row, so the pick at row 1 clamps.
    app.input_ui.set_text("/agents ".to_string());
    app.input_ui.set_cursor(app.input_ui.text().chars().count());
    app.input_changed();

    assert_eq!(app.input_ui.command_selection(), 0);
    assert!(!app.input_ui.command_selection_explicit());
}

// Covers: MCP argument rows obey the same pick rule; without an arrow-key
// pick Enter submits the command as typed.
// Owner: command palette Enter policy
#[test]
fn enter_on_mcp_argument_rows_follows_the_pick_rule() {
    let choice = CommandChoice {
        name: "alice".into(),
        usage: "alice".into(),
        description: String::new(),
        kind: CommandChoiceKind::McpPromptArgument { value: 0..0 },
    };

    let mut app = test_app();
    assert!(!app.enter_completes_choice(&choice));

    app.input_ui.move_command_selection(1);
    assert!(app.enter_completes_choice(&choice));
}
