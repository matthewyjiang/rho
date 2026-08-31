use super::super::{
    tests::test_app, CommandChoice, CommandChoiceKind, HistoryDirection, InputSubmissionMode,
};

#[test]
fn completing_goal_command_reveals_lifecycle_actions() {
    let mut app = test_app();
    app.input_ui.set_text("/goal".to_string());
    app.input_ui.set_cursor(app.input_ui.text().chars().count());
    app.clamp_command_selection();

    let goal = app.selected_command().unwrap();
    app.complete_command_choice(&goal);

    assert_eq!(app.input_ui.text(), "/goal ");
    assert!(app.command_palette_visible());
    let matches = app.command_matches();
    assert_eq!(
        matches
            .iter()
            .map(|choice| choice.usage.as_str())
            .collect::<Vec<_>>(),
        vec!["/goal resume", "/goal clear"]
    );

    app.input_ui
        .set_text("/goal release is published".to_string());
    app.input_ui.set_cursor(app.input_ui.text().chars().count());
    app.input_changed();
    assert!(!app.command_palette_visible());
}

#[test]
fn goal_lifecycle_action_completion_replaces_placeholder() {
    let mut app = test_app();
    app.input_ui.set_text("/goal ".to_string());
    app.input_ui.set_cursor(app.input_ui.text().chars().count());
    app.clamp_command_selection();
    app.input_ui.move_command_selection(1);

    let clear = app.selected_command().unwrap();
    app.complete_command_choice(&clear);

    assert_eq!(app.input_ui.text(), "/goal clear");
    assert_eq!(app.input_ui.cursor(), "/goal clear".chars().count());
    assert_eq!(
        app.input_ui.submission_mode(),
        InputSubmissionMode::ParseCommands
    );
}

#[test]
fn exact_template_match_precedes_builtin_prefix_match() {
    let mut app = test_app();
    app.info
        .runtime
        .prompt_templates
        .insert("mod".into(), "custom template".into());
    app.input_ui.set_text("/mod argument".to_string());
    app.input_ui.set_cursor(4);

    let matches = app.command_matches();

    assert_eq!(matches[0].name, "prompt:mod");
    assert_eq!(matches[1].name, "model");
}

#[test]
fn template_completion_expands_pasted_arguments_and_clears_segments() {
    let mut app = test_app();
    app.info
        .runtime
        .prompt_templates
        .insert("review".into(), "Review this:".into());
    app.insert_input_text("/review ");
    app.insert_pasted_input_text("alpha\nbeta");
    let choice = app.selected_command().unwrap();

    app.complete_command_choice(&choice);

    assert_eq!(app.input_ui.text(), "Review this: alpha\nbeta ");
    assert_eq!(app.expanded_input(), "Review this: alpha\nbeta ");
    assert!(app.input_ui.paste_segments().is_empty());
}

#[test]
fn template_completion_marks_slash_prefixed_contents_as_prompt() {
    let mut app = test_app();
    app.info
        .runtime
        .prompt_templates
        .insert("review".into(), "/diff literally".into());
    app.input_ui.set_text("/review".to_string());
    app.input_ui.set_cursor(app.input_char_len());
    let choice = app.selected_command().unwrap();

    app.complete_command_choice(&choice);

    assert_eq!(app.input_ui.text(), "/diff literally ");
    assert_eq!(app.input_ui.submission_mode(), InputSubmissionMode::Prompt);
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

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input_ui.text(), "/model");
    assert!(!app.command_palette_visible());

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert_eq!(app.input_ui.text(), "/info");
    assert!(!app.command_palette_visible());

    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
    app.recall_input_history_or_move_cursor(HistoryDirection::Next, 80);
    assert_eq!(app.input_ui.text(), "/c");
    assert!(app.command_palette_visible());

    app.recall_input_history_or_move_cursor(HistoryDirection::Previous, 80);
    assert!(!app.command_palette_visible());
    app.backspace_input();
    assert_eq!(app.input_ui.text(), "/mode");
    assert!(app.command_palette_visible());
}

// Covers: Esc on a bare slash must not leave a trap for blind retyping into //cmd.
// Owner: command palette dismiss policy (idle and during-turn Esc arms)
#[test]
fn dismiss_on_esc_clears_bare_slash_composer() {
    let mut app = test_app();
    app.input_ui.set_text("/".to_string());
    app.input_ui.set_cursor(1);
    assert!(app.command_palette_visible());

    app.dismiss_command_palette_on_esc();

    assert_eq!(app.input_ui.text(), "");
    assert_eq!(app.input_ui.cursor(), 0);
    assert!(app.input_ui.command_palette_dismissed());
}

// Covers: Esc on a partial slash command keeps typed content in the composer.
// Owner: command palette dismiss policy (idle and during-turn Esc arms)
#[test]
fn dismiss_on_esc_preserves_partial_slash_command() {
    let mut app = test_app();
    app.input_ui.set_text("/mo".to_string());
    app.input_ui.set_cursor(3);
    assert!(app.command_palette_visible());

    app.dismiss_command_palette_on_esc();

    assert_eq!(app.input_ui.text(), "/mo");
    assert_eq!(app.input_ui.cursor(), 3);
    assert!(app.input_ui.command_palette_dismissed());
}

// Covers: with argument rows open, Enter must not spend the default
// highlight on a row nobody picked; an arrow-key pick enables it, and a
// changed question takes the pick back.
// Owner: command palette Enter policy
#[test]
fn enter_keeps_bare_command_until_an_argument_row_is_picked() {
    let mut app = test_app();
    app.input_ui.set_text("/goal ".to_string());
    app.input_ui.set_cursor(app.input_ui.text().chars().count());
    app.clamp_command_selection();
    let matches = app.command_matches();

    assert!(!app.enter_completes_choice(&matches[0]));

    app.input_ui.move_command_selection(1);
    assert!(app.enter_completes_choice(&matches[1]));

    // Moving the cursor back into the command token changes the palette
    // question, so the pick does not survive it.
    app.input_ui.set_text("/goal".to_string());
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
    app.input_ui.set_text("/goal ".to_string());
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
// pick (or a typed value narrowing the server's suggestions) Enter submits
// the command as typed. A non-empty typed value is the exception that lets
// Enter complete without a pick.
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

    // Typed non-empty value under the cursor enables Enter without a pick.
    // Removing the typed-value side of enter_completes_choice must fail this.
    let mut app = test_app();
    app.mcp_catalog.insert_offline_prompt(
        crate::tools::mcp::catalog::McpPrompt {
            server: "tickets".into(),
            name: "triage".into(),
            title: None,
            description: None,
            arguments: vec![
                crate::tools::mcp::catalog::McpPromptArgument {
                    name: "severity".into(),
                    description: None,
                    required: false,
                },
                crate::tools::mcp::catalog::McpPromptArgument {
                    name: "owner".into(),
                    description: None,
                    required: false,
                },
            ],
        },
        true,
    );
    let typed = "/mcp:tickets:triage owner=al";
    app.input_ui.set_text(typed.to_string());
    app.input_ui.set_cursor(typed.chars().count());
    assert!(!app.input_ui.command_selection_explicit());
    assert!(app
        .mcp_argument_cursor()
        .is_some_and(|cursor| cursor.key.typed == "al"));
    assert!(app.enter_completes_choice(&choice));

    // An empty value still needs an explicit pick.
    let empty = "/mcp:tickets:triage owner=";
    app.input_ui.set_text(empty.to_string());
    app.input_ui.set_cursor(empty.chars().count());
    assert!(app
        .mcp_argument_cursor()
        .is_some_and(|cursor| cursor.key.typed.is_empty()));
    assert!(!app.enter_completes_choice(&choice));
}
