use super::super::{tests::test_app, InputSubmissionMode};

#[test]
fn exact_template_match_precedes_builtin_prefix_match() {
    let mut app = test_app();
    app.info
        .prompt_templates
        .insert("mod".into(), "custom template".into());
    app.input = "/mod argument".into();
    app.input_cursor = 4;

    let matches = app.command_matches();

    assert_eq!(matches[0].name, "prompt:mod");
    assert_eq!(matches[1].name, "model");
}

#[test]
fn template_completion_expands_pasted_arguments_and_clears_segments() {
    let mut app = test_app();
    app.info
        .prompt_templates
        .insert("review".into(), "Review this:".into());
    app.insert_input_text("/review ");
    app.insert_pasted_input_text("alpha\nbeta");
    let choice = app.selected_command().unwrap();

    app.complete_command_choice(&choice);

    assert_eq!(app.input, "Review this: alpha\nbeta ");
    assert_eq!(app.expanded_input(), "Review this: alpha\nbeta ");
    assert!(app.paste_segments.is_empty());
}

#[test]
fn template_completion_marks_slash_prefixed_contents_as_prompt() {
    let mut app = test_app();
    app.info
        .prompt_templates
        .insert("review".into(), "/diff literally".into());
    app.input = "/review".into();
    app.input_cursor = app.input_char_len();
    let choice = app.selected_command().unwrap();

    app.complete_command_choice(&choice);

    assert_eq!(app.input, "/diff literally ");
    assert_eq!(app.input_submission_mode, InputSubmissionMode::Prompt);
}
