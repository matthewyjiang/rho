use super::*;
use crate::questionnaire::QuestionnaireDefaultSelection;

fn text_question() -> QuestionnaireQuestion {
    QuestionnaireQuestion {
        id: "file".into(),
        question: "Which file?".into(),
        header: None,
        help: None,
        default: None,
        default_selection: QuestionnaireDefaultSelection::Selected,
        kind: QuestionnaireQuestionKind::Text,
        required: true,
        choices: Vec::new(),
        allow_other: false,
    }
}

#[test]
fn cancel_sends_user_cancelled_reply() {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut composer = QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![text_question()],
        },
        QuestionnaireResponseChannel::new(reply_tx),
    );

    composer.cancel_by_user();

    assert!(matches!(
        reply_rx.try_recv(),
        Ok(QuestionnaireReply::Cancelled(
            QuestionnaireCancelReason::UserCancelled
        ))
    ));
}

#[test]
fn submit_sends_selection_answers() {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut composer = QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: Some("PR details".into()),
            reason: Some("Need missing preferences".into()),
            questions: vec![
                QuestionnaireQuestion {
                    id: "branch".into(),
                    question: "Which branch?".into(),
                    header: None,
                    help: None,
                    default: Some(serde_json::json!("main")),
                    default_selection: QuestionnaireDefaultSelection::Selected,
                    kind: QuestionnaireQuestionKind::Choice,
                    required: true,
                    choices: vec!["main".into(), "develop".into()],
                    allow_other: true,
                },
                QuestionnaireQuestion {
                    id: "test_suites".into(),
                    question: "Which test suites should I run?".into(),
                    header: None,
                    help: None,
                    default: Some(serde_json::json!(["unit"])),
                    default_selection: QuestionnaireDefaultSelection::Selected,
                    kind: QuestionnaireQuestionKind::MultiSelect,
                    required: true,
                    choices: vec!["unit".into(), "e2e".into(), "lint".into()],
                    allow_other: false,
                },
                QuestionnaireQuestion {
                    id: "apply".into(),
                    question: "Apply changes?".into(),
                    header: None,
                    help: None,
                    default: Some(serde_json::json!("yes")),
                    default_selection: QuestionnaireDefaultSelection::Selected,
                    kind: QuestionnaireQuestionKind::Confirm,
                    required: true,
                    choices: Vec::new(),
                    allow_other: false,
                },
            ],
        },
        QuestionnaireResponseChannel::new(reply_tx),
    );
    composer.fields[0].selection = FieldSelection::Other;
    composer.fields[0].choice_cursor = 2;
    composer.fields[0].other_value = "release".into();
    composer.fields[0].other_cursor = "release".chars().count();
    composer.fields[1].selection = FieldSelection::Multi {
        selected: vec![0, 1],
        other: false,
    };
    composer.fields[2].selection = FieldSelection::Single(1);

    let submitted = composer.submit().unwrap();

    assert!(
        submitted.display.contains("Which branch?: release"),
        "{}",
        submitted.display
    );
    assert!(
        submitted
            .display
            .contains("Which test suites should I run?: unit, e2e"),
        "{}",
        submitted.display
    );
    assert!(
        submitted.display.contains("Apply changes?: no"),
        "{}",
        submitted.display
    );
    assert!(matches!(
        reply_rx.try_recv(),
        Ok(QuestionnaireReply::Answer(QuestionnaireResponse { answers }))
            if answers == vec![
                QuestionnaireAnswer { id: "branch".into(), answer: serde_json::json!("release") },
                QuestionnaireAnswer { id: "test_suites".into(), answer: serde_json::json!(["unit", "e2e"]) },
                QuestionnaireAnswer { id: "apply".into(), answer: serde_json::json!("no") },
            ]
    ));
}

#[test]
fn required_confirm_without_default_requires_explicit_choice() {
    let question = QuestionnaireQuestion {
        id: "apply".into(),
        question: "Apply changes?".into(),
        header: None,
        help: None,
        default: None,
        default_selection: QuestionnaireDefaultSelection::Selected,
        kind: QuestionnaireQuestionKind::Confirm,
        required: true,
        choices: Vec::new(),
        allow_other: false,
    };
    let field = QuestionnaireFieldState::new(&question);

    assert_eq!(field.selection, FieldSelection::None);
    assert_eq!(
        normalize_questionnaire_answer(&question, &field),
        Err("answer is not selected".into())
    );

    let mut field = field;
    field.toggle_highlighted(&question);
    assert_eq!(
        normalize_questionnaire_answer(&question, &field),
        Ok(serde_json::json!("yes"))
    );
}

#[test]
fn multi_select_default_preserves_commas() {
    let question = QuestionnaireQuestion {
        id: "targets".into(),
        question: "Targets?".into(),
        header: None,
        help: None,
        default: Some(serde_json::json!(["New York, NY", "Los Angeles, CA"])),
        default_selection: QuestionnaireDefaultSelection::Selected,
        kind: QuestionnaireQuestionKind::MultiSelect,
        required: true,
        choices: vec!["New York, NY".into(), "Boston, MA".into()],
        allow_other: true,
    };

    let field = QuestionnaireFieldState::new(&question);

    assert_eq!(
        field.selection,
        FieldSelection::Multi {
            selected: vec![0],
            other: true
        }
    );
    assert_eq!(field.other_value, "Los Angeles, CA");
    assert_eq!(
        normalize_questionnaire_answer(&question, &field),
        Ok(serde_json::json!(["New York, NY", "Los Angeles, CA"]))
    );
}

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn form_composer() -> QuestionnaireComposer {
    QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: Some("PR details".into()),
            reason: Some("Need missing preferences".into()),
            questions: vec![
                QuestionnaireQuestion {
                    id: "branch".into(),
                    question: "Which branch?".into(),
                    header: None,
                    help: None,
                    default: Some(serde_json::json!("main")),
                    default_selection: QuestionnaireDefaultSelection::Selected,
                    kind: QuestionnaireQuestionKind::Choice,
                    required: true,
                    choices: vec!["main".into(), "develop".into()],
                    allow_other: true,
                },
                QuestionnaireQuestion {
                    id: "suites".into(),
                    question: "Which suites?".into(),
                    header: None,
                    help: None,
                    default: None,
                    default_selection: QuestionnaireDefaultSelection::Selected,
                    kind: QuestionnaireQuestionKind::MultiSelect,
                    required: true,
                    choices: vec!["unit".into(), "e2e".into()],
                    allow_other: false,
                },
            ],
        },
        QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
    )
}

#[test]
fn frame_shows_wrapped_choice_descriptions_below_labels() {
    let question = QuestionnaireQuestion {
        id: "style".into(),
        question: "Style?".into(),
        header: None,
        help: None,
        default: None,
        default_selection: QuestionnaireDefaultSelection::Selected,
        kind: QuestionnaireQuestionKind::Choice,
        required: true,
        choices: vec![
            QuestionnaireChoice::new("brief", "Brief").description("A short answer with key facts"),
            QuestionnaireChoice::new("detailed", "Detailed"),
        ],
        allow_other: false,
    };
    let composer = QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![question],
        },
        QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
    );

    let (lines, cursor) = questionnaire_frame(&composer, 24);
    let text = lines.iter().map(line_text).collect::<Vec<_>>();
    let label_row = text
        .iter()
        .position(|line| line.contains("→ ○ Brief"))
        .expect("choice label");

    assert_eq!(cursor.y as usize, label_row);
    assert_eq!(text[label_row + 1].trim_end(), "        A short answer");
    assert_eq!(text[label_row + 2].trim_end(), "        with key facts");
    assert!(text.iter().any(|line| line.contains("○ Detailed")));
}

#[test]
fn frame_marks_focused_default_as_recommended_without_selecting() {
    let question = QuestionnaireQuestion {
        id: "prompt".into(),
        question: "Prompt mode?".into(),
        header: None,
        help: None,
        default: Some(serde_json::json!("extend")),
        default_selection: QuestionnaireDefaultSelection::Focused,
        kind: QuestionnaireQuestionKind::Choice,
        required: true,
        choices: vec![
            QuestionnaireChoice::new("replace", "replace"),
            QuestionnaireChoice::new("extend", "extend"),
        ],
        allow_other: false,
    };
    let composer = QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![question],
        },
        QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
    );

    let (lines, cursor) = questionnaire_frame(&composer, 48);
    let text = lines.iter().map(line_text).collect::<Vec<_>>();
    let recommended_row = text
        .iter()
        .position(|line| line.contains("extend (recommended)"))
        .expect("recommended choice");

    assert_eq!(cursor.y as usize, recommended_row);
    assert!(text[recommended_row].contains('→'));
    assert!(text[recommended_row].contains('○'));
    assert!(text.iter().any(|line| line.contains("○ replace")));
    assert!(!text.iter().any(|line| line.contains('●')));
}

#[test]
fn frame_marks_focused_custom_other_without_selecting() {
    let question = QuestionnaireQuestion {
        id: "city".into(),
        question: "Which city?".into(),
        header: None,
        help: None,
        default: Some(serde_json::json!("Portland")),
        default_selection: QuestionnaireDefaultSelection::Focused,
        kind: QuestionnaireQuestionKind::Choice,
        required: true,
        choices: vec![
            QuestionnaireChoice::new("nyc", "New York"),
            QuestionnaireChoice::new("la", "Los Angeles"),
        ],
        allow_other: true,
    };
    let composer = QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![question],
        },
        QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
    );

    let (lines, cursor) = questionnaire_frame(&composer, 48);
    let text = lines.iter().map(line_text).collect::<Vec<_>>();
    let recommended_row = text
        .iter()
        .position(|line| line.contains("other: Portland (recommended)"))
        .expect("focused other");

    assert_eq!(cursor.y as usize, recommended_row);
    assert!(text[recommended_row].contains('→'));
    assert!(text[recommended_row].contains('○'));
    assert!(text.iter().any(|line| line.contains("○ New York")));
    assert!(!text.iter().any(|line| line.contains('●')));
}

#[test]
fn cursor_lands_on_highlighted_choice_row_after_wrapping() {
    let question = QuestionnaireQuestion {
        id: "style".into(),
        question: "hello wide world".into(),
        header: None,
        help: None,
        default: None,
        default_selection: QuestionnaireDefaultSelection::Selected,
        kind: QuestionnaireQuestionKind::Choice,
        required: true,
        choices: vec!["brief".into(), "detailed".into()],
        allow_other: false,
    };
    let field = QuestionnaireFieldState::new(&question);
    let (lines, cursor) = questionnaire_frame(
        &QuestionnaireComposer {
            request: QuestionnaireRequest {
                title: None,
                reason: None,
                questions: vec![question.clone()],
            },
            response: QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
            fields: vec![field],
            active_index: 0,
        },
        14,
    );

    let highlighted_row = lines
        .iter()
        .position(|line| line_text(line).contains('→'))
        .expect("highlighted choice row");
    assert_eq!(cursor.y as usize, highlighted_row);
    assert_eq!(cursor.x, 2);
    assert!(line_text(&lines[highlighted_row]).contains("○ brief"));
}

#[test]
fn frame_shows_tab_bar_with_answered_marks_and_active_question() {
    let mut composer = form_composer();
    composer.active_index = 1;
    let (lines, _) = questionnaire_frame(&composer, 60);
    let text = lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(text[0], "PR details");
    assert_eq!(text[1], "Need missing preferences");
    let tabs = text
        .iter()
        .find(|line| line.contains('│'))
        .expect("tab bar");
    assert!(tabs.contains("1 Which branch? ✓"), "{tabs}");
    assert!(tabs.contains("2 Which suites?"), "{tabs}");
    assert!(
        !tabs.contains("Which suites? ✓"),
        "unanswered tab must not be checked: {tabs}"
    );
    assert!(
        !text.iter().any(|line| line.contains("develop")),
        "inactive question choices are not rendered"
    );
    assert!(text.iter().any(|line| line.contains("▸ 2. Which suites?")));
    assert!(text.iter().any(|line| line.contains("→ □ unit")));
    assert!(
        text.last().unwrap().contains("space toggle"),
        "footer is contextual for multi_select: {}",
        text.last().unwrap()
    );
}

fn many_questions_composer(count: usize) -> QuestionnaireComposer {
    QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: (0..count)
                .map(|index| QuestionnaireQuestion {
                    id: format!("q{}", index + 1),
                    question: format!("Question number {}?", index + 1),
                    header: None,
                    help: None,
                    default: None,
                    default_selection: QuestionnaireDefaultSelection::Selected,
                    kind: QuestionnaireQuestionKind::Confirm,
                    required: true,
                    choices: Vec::new(),
                    allow_other: false,
                })
                .collect(),
        },
        QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
    )
}

fn tab_bar_line(composer: &QuestionnaireComposer, width: usize) -> String {
    let (lines, _) = questionnaire_frame(composer, width);
    lines
        .iter()
        .map(line_text)
        .find(|line| line.contains('│'))
        .expect("tab bar")
}

#[test]
fn tab_bar_scrolls_to_keep_active_chip_visible() {
    let mut composer = many_questions_composer(8);

    let tabs = tab_bar_line(&composer, 60);
    assert!(tabs.contains("1 Question number"), "{tabs}");
    assert!(!tabs.starts_with('…'), "{tabs}");
    assert!(
        tabs.ends_with('…'),
        "hidden right chips need an indicator: {tabs}"
    );

    composer.active_index = 7;
    let tabs = tab_bar_line(&composer, 60);
    assert!(
        tabs.starts_with('…'),
        "hidden left chips need an indicator: {tabs}"
    );
    assert!(tabs.contains("8 Question number"), "{tabs}");

    composer.active_index = 4;
    let tabs = tab_bar_line(&composer, 60);
    assert!(tabs.contains("5 Question number"), "{tabs}");
    assert!(tabs.starts_with('…') && tabs.ends_with('…'), "{tabs}");
}

#[test]
fn tab_bar_stays_on_one_row() {
    let mut composer = many_questions_composer(8);
    for active in 0..8 {
        composer.active_index = active;
        let (lines, _) = questionnaire_frame(&composer, 40);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();
        // No title or reason: the frame is tab bar, blank, active question, …
        assert!(
            text[0].contains(&format!("{} Question number", active + 1)),
            "active chip visible on the single tab row: {}",
            text[0]
        );
        assert_eq!(text[1], "", "active={active}");
        assert!(text[2].starts_with('▸'), "active={active}: {}", text[2]);
    }
}

#[test]
fn footer_offers_next_before_the_last_question_and_submit_on_it() {
    let mut composer = form_composer();

    let (lines, _) = questionnaire_frame(&composer, 80);
    let footer = line_text(lines.last().unwrap());
    assert!(footer.contains("enter next"), "{footer}");
    assert!(!footer.contains("enter submit"), "{footer}");

    composer.active_index = 1;
    let (lines, _) = questionnaire_frame(&composer, 80);
    let footer = line_text(lines.last().unwrap());
    assert!(footer.contains("enter submit"), "{footer}");
}

#[test]
fn failed_submit_jumps_to_the_offending_question() {
    let mut composer = form_composer();
    // Question 1 has a default answer; question 2 (multi_select, required)
    // is unanswered. Submit from the last question.
    composer.active_index = 1;

    let error = composer.submit().unwrap_err();

    assert!(error.starts_with("question 2:"), "{error}");
    assert_eq!(composer.active_index, 1);

    // Clear question 1's answer as well: the jump targets the first failure.
    composer.active_index = 0;
    composer.clear_active_answer();
    composer.active_index = 1;
    let error = composer.submit().unwrap_err();

    assert!(error.starts_with("question 1:"), "{error}");
    assert_eq!(composer.active_index, 0);
}

#[test]
fn tab_chips_prefer_question_headers() {
    let mut composer = form_composer();
    composer.request.questions[0].header = Some("Branch".into());
    let tabs = tab_bar_line(&composer, 60);

    assert!(tabs.contains("1 Branch ✓"), "{tabs}");
    assert!(!tabs.contains("Which branch?"), "{tabs}");
    assert!(
        tabs.contains("2 Which suites?"),
        "questions without a header fall back to the question text: {tabs}"
    );
}

#[test]
fn single_question_renders_without_tab_bar_or_header() {
    let composer = QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![text_question()],
        },
        QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
    );
    let (lines, _) = questionnaire_frame(&composer, 60);
    let text = lines.iter().map(line_text).collect::<Vec<_>>();

    assert!(text[0].starts_with("▸ Which file?"), "{}", text[0]);
    assert!(!text.iter().any(|line| line.contains('│')));
}

#[test]
fn arrow_navigation_flows_across_questions() {
    let mut composer = form_composer();
    assert_eq!(composer.active_index, 0);

    composer.move_down(); // main -> develop
    composer.move_down(); // develop -> other
    assert_eq!(composer.active_index, 0);
    composer.move_down(); // other row is last -> next question
    assert_eq!(composer.active_index, 1);

    composer.move_up(); // first choice of q2 -> previous question
    assert_eq!(composer.active_index, 0);
}

#[test]
fn word_navigation_and_deletion_stay_with_composer_state() {
    let mut composer = QuestionnaireComposer::new(
        QuestionnaireRequest {
            title: None,
            reason: None,
            questions: vec![text_question()],
        },
        QuestionnaireResponseChannel::new(tokio::sync::oneshot::channel().0),
    );
    composer.insert_text("alpha beta");

    composer.move_text_cursor_previous_word();
    assert_eq!(composer.active_field().other_cursor, 6);
    composer.delete_previous_word();
    assert_eq!(composer.active_field().other_value, "beta");
}
