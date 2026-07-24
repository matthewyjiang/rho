use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};

use crate::{
    questionnaire::{QuestionnaireDefaultSelection, QuestionnaireQuestionKind},
    tui::questionnaire::{QuestionnaireQuestion, QuestionnaireRequest},
};

use super::*;

fn rendered_activity(app: &App) -> String {
    let status = app.activity_status().expect("activity is visible");
    app.turn
        .loading_spinner()
        .line(Instant::now(), 80, status)
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn streamed_events_update_the_rendered_activity_phase() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    app.handle_agent_event(ViewModelEvent::StepStarted(1), &mut terminal)
        .unwrap();
    assert!(rendered_activity(&app).contains("waiting for provider"));

    app.handle_agent_event(
        ViewModelEvent::ReasoningDelta("thinking".into()),
        &mut terminal,
    )
    .unwrap();
    assert!(rendered_activity(&app).contains("thinking"));

    app.handle_agent_event(ViewModelEvent::OutputDelta("answer".into()), &mut terminal)
        .unwrap();
    assert!(rendered_activity(&app).contains("responding"));

    app.handle_agent_event(ViewModelEvent::ProviderStreamReset, &mut terminal)
        .unwrap();
    assert!(rendered_activity(&app).contains("retrying provider"));
}

#[test]
fn provider_stream_reset_clears_attempt_owned_tool_previews() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    app.handle_agent_event(
        ViewModelEvent::ToolCallUpdated {
            index: 0,
            call_id: Some(rho_sdk::ToolCallId::from_string("stale-call").unwrap()),
            display_lines: vec!["stale preview".into()],
        },
        &mut terminal,
    )
    .unwrap();
    assert_eq!(app.turn.tool_calls().live_entries().count(), 1);

    app.handle_agent_event(ViewModelEvent::ProviderStreamReset, &mut terminal)
        .unwrap();

    assert_eq!(app.turn.tool_calls().live_entries().count(), 0);
}

#[test]
fn questionnaire_phase_is_a_temporary_overlay_on_tool_activity() {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut app = test_app();
    app.begin_provider_turn_ui();
    app.turn.set_activity_phase(ActivityPhase::RunningTool);
    app.input_ui
        .set_composer(ComposerMode::Questionnaire(QuestionnaireComposer::new(
            QuestionnaireRequest {
                title: None,
                reason: None,
                questions: vec![QuestionnaireQuestion {
                    id: "continue".into(),
                    question: "Continue?".into(),
                    header: None,
                    help: None,
                    default: None,
                    default_selection: QuestionnaireDefaultSelection::Selected,
                    kind: QuestionnaireQuestionKind::Confirm,
                    required: true,
                    choices: Vec::new(),
                    allow_other: false,
                }],
            },
            QuestionnaireResponseChannel::new(reply_tx),
        )));

    assert!(rendered_activity(&app).contains("waiting for input"));

    app.handle_questionnaire_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    assert!(matches!(app.input_ui.composer(), ComposerMode::Input));
    assert!(reply_rx.try_recv().is_ok());
    assert!(rendered_activity(&app).contains("running tool"));
}
