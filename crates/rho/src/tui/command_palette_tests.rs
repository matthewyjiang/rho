use super::super::{tests::test_app, InputSubmissionMode};

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn goal_usage_is_not_truncated_when_space_is_available() {
    let mut app = test_app();
    app.input_ui.set_text("/goal".to_string());
    app.input_ui.set_cursor(app.input_ui.text().chars().count());
    app.clamp_command_selection();

    let rendered = app
        .command_suggestion_lines(80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered.contains("/goal [condition|resume|clear]"),
        "{rendered}"
    );
    assert!(rendered.contains("/goal resume"), "{rendered}");
    assert!(rendered.contains("/goal clear"), "{rendered}");
    assert!(
        rendered.contains("show status or work until a condition is met"),
        "{rendered}"
    );
}

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
    app.input_ui.set_command_selection(1);

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
