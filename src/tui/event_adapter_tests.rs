use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ModelUsage, ToolCall},
    tool::{OperationKind, ToolMetadata, ToolOutput},
    HostChoice, HostInputRequest, HostQuestion, RunEvent, SelectionMode, ToolCallId,
    ToolCompletion,
};

use super::{host_response, questionnaire_request, SdkEventAdapter, ViewEvent, ViewModelEvent};
use crate::questionnaire::{QuestionnaireAnswer, QuestionnaireResponse};

#[test]
fn translates_streaming_and_usage_events_without_rendering_state() {
    let mut adapter = SdkEventAdapter::default();

    assert!(matches!(
        adapter.translate(RunEvent::AssistantTextDelta { text: "hello".into() }),
        ViewEvent::Update(ViewModelEvent::OutputDelta(text)) if text == "hello"
    ));
    let usage = ModelUsage {
        output_tokens: Some(3),
        ..ModelUsage::default()
    };
    assert!(matches!(
        adapter.translate(RunEvent::UsageUpdated { usage: usage.clone() }),
        ViewEvent::Update(ViewModelEvent::Usage(translated)) if translated == usage
    ));
}

#[test]
fn retains_structured_tool_metadata_until_completion() {
    let mut adapter = SdkEventAdapter::default();
    let call_id = ToolCallId::from_string("call-1").unwrap();
    let call = ToolCall {
        id: call_id.to_string(),
        name: "edit_file".into(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
    };
    let _ = adapter.translate(RunEvent::ToolProposed { call });
    let _ = adapter.translate(RunEvent::ToolStarted {
        call_id: call_id.clone(),
        name: "edit_file".into(),
        metadata: ToolMetadata::new().operation(OperationKind::Write),
    });
    let output = ToolOutput::text("updated").metadata(
        ToolMetadata::new()
            .affected_path("src/lib.rs")
            .diff("--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n"),
    );

    let ViewEvent::Update(ViewModelEvent::ToolFinished {
        ok, display_lines, ..
    }) = adapter.translate(RunEvent::ToolFinished {
        call_id,
        result: ToolCompletion::Success(output),
    })
    else {
        panic!("expected translated tool completion");
    };

    assert!(ok);
    assert_eq!(display_lines, vec!["edit_file src/lib.rs", "-old\n+new"]);
}

#[test]
fn converts_questionnaires_and_answers_through_typed_sdk_values() {
    let question = HostQuestion::new(
        "language",
        "Language?",
        vec![HostChoice::new("rust", "Rust"), HostChoice::new("go", "Go")],
        SelectionMode::One,
    )
    .unwrap()
    .help("Choose one");
    let request = HostInputRequest::questionnaire("Setup", vec![question]).unwrap();

    let translated = questionnaire_request(&request);
    assert_eq!(translated.title.as_deref(), Some("Setup"));
    assert_eq!(translated.questions[0].choices, vec!["Rust", "Go"]);

    let response = host_response(QuestionnaireResponse {
        answers: vec![QuestionnaireAnswer {
            id: "language".into(),
            answer: serde_json::Value::String("rust".into()),
        }],
    });
    assert_eq!(response.answers()["language"], vec!["rust"]);
}
