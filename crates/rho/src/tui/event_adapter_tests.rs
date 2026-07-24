use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ModelUsage, ToolCall},
    tool::{OperationKind, ToolAsset, ToolMetadata, ToolOutput},
    HostChoice, HostInputRequest, HostQuestion, ProviderStreamResetReason, Revision, RunEvent,
    RunId, SelectionMode, ToolCallId, ToolCompletion,
};

use super::{host_response, questionnaire_request, SdkEventAdapter, ViewEvent, ViewModelEvent};
use crate::{
    questionnaire::{QuestionnaireQuestionKind, QuestionnaireResponse},
    tui::questionnaire::{
        QuestionnaireChoice, QuestionnaireComposer, QuestionnaireReply,
        QuestionnaireResponseChannel,
    },
};

#[test]
fn translates_streaming_and_usage_events_without_rendering_state() {
    let mut adapter = SdkEventAdapter::default();

    assert!(matches!(
        adapter.translate(RunEvent::Started {
            run_id: RunId::new(),
            revision: Revision::INITIAL,
        }),
        ViewEvent::Update(ViewModelEvent::RunStarted)
    ));
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
fn provider_diagnostics_are_shown_in_interactive_failures() {
    let mut adapter = SdkEventAdapter::default();

    let event = adapter.translate(RunEvent::ProviderDiagnostic {
        detail: rho_sdk::ProviderDiagnostic::new("{\"error\":\"bad request\"}"),
    });

    let ViewEvent::Notice(message) = event else {
        panic!("expected diagnostic notice");
    };
    assert_eq!(message, "provider diagnostic:\n{\"error\":\"bad request\"}");
}

#[test]
fn provider_retry_resets_the_current_provider_stream() {
    let mut adapter = SdkEventAdapter::default();

    for reason in [
        ProviderStreamResetReason::InvalidResponse,
        ProviderStreamResetReason::RetryableFailure(rho_sdk::ProviderErrorKind::Unavailable),
    ] {
        assert!(matches!(
            adapter.translate(RunEvent::ProviderStreamReset {
                reason,
                detail: "retrying".into(),
            }),
            ViewEvent::Update(ViewModelEvent::ProviderStreamReset)
        ));
    }
}

#[test]
fn physical_provider_retry_maps_to_typed_view_model_event() {
    let mut adapter = SdkEventAdapter::default();

    assert!(matches!(
        adapter.translate(RunEvent::ProviderActivity {
            kind: rho_sdk::PROVIDER_ACTIVITY_REQUEST_RETRY.into(),
            detail: "retrying".into(),
        }),
        ViewEvent::Update(ViewModelEvent::ProviderRetry)
    ));
}

#[test]
fn legacy_malformed_response_activity_does_not_reset_twice() {
    let mut adapter = SdkEventAdapter::default();

    assert!(matches!(
        adapter.translate(RunEvent::ProviderActivity {
            kind: rho_sdk::PROVIDER_ACTIVITY_INVALID_RESPONSE_RETRY.into(),
            detail: "retrying".into(),
        }),
        ViewEvent::Ignored
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
        call_id: translated_call_id,
        ok,
        display_lines,
        ..
    }) = adapter.translate(RunEvent::ToolFinished {
        call_id: call_id.clone(),
        result: ToolCompletion::Success(output),
    })
    else {
        panic!("expected translated tool completion");
    };

    assert_eq!(translated_call_id, call_id);
    assert!(ok);
    assert_eq!(display_lines, vec!["edit_file src/lib.rs", "-old\n+new"]);
}

#[test]
fn forwards_image_asset_on_tool_completion() {
    let mut adapter = SdkEventAdapter::default();
    let call_id = ToolCallId::from_string("call-image").unwrap();
    let _ = adapter.translate(RunEvent::ToolStarted {
        call_id: call_id.clone(),
        name: "read_file".into(),
        metadata: ToolMetadata::new(),
    });
    let asset = ToolAsset::new("image/png", vec![1, 2, 3, 4]);
    let output = ToolOutput::text("image/png image (4 bytes)").metadata(
        ToolMetadata::new()
            .asset(asset.clone())
            .presentation_notice("image preview unavailable: invalid image"),
    );

    let ViewEvent::Update(ViewModelEvent::ToolFinished {
        call_id: translated_call_id,
        image_asset,
        display_lines,
        ..
    }) = adapter.translate(RunEvent::ToolFinished {
        call_id: call_id.clone(),
        result: ToolCompletion::Success(output),
    })
    else {
        panic!("expected translated tool completion");
    };

    assert_eq!(translated_call_id, call_id);
    assert_eq!(image_asset, Some(asset));
    assert_eq!(
        display_lines,
        ["read_file ", "image preview unavailable: invalid image"]
    );
}

#[test]
fn choice_round_trip_renders_label_and_returns_machine_value() {
    let question = HostQuestion::new(
        "language",
        "Language?",
        vec![
            HostChoice::new("rust", "Rust").description("Strong type and memory safety"),
            HostChoice::new("go", "Go"),
        ],
        SelectionMode::One,
    )
    .unwrap()
    .help("Choose one");
    let request = HostInputRequest::questionnaire("Setup", vec![question]).unwrap();
    let translated = questionnaire_request(&request);

    assert_eq!(translated.title.as_deref(), Some("Setup"));
    assert_eq!(
        translated.questions[0].choices,
        vec![
            QuestionnaireChoice::new("rust", "Rust").description("Strong type and memory safety"),
            QuestionnaireChoice::new("go", "Go"),
        ]
    );

    let (response, display) = submit(translated, |composer| composer.toggle_active_choice());
    let host = host_response(response);

    assert_eq!(display, "Rust");
    assert_eq!(host.answers()["language"], ["rust"]);
    assert!(request.validate(&host).is_ok());
}

#[test]
fn focused_default_round_trips_without_preselecting() {
    use rho_sdk::DefaultSelection;

    let question = HostQuestion::new(
        "prompt",
        "Prompt mode?",
        vec![
            HostChoice::new("replace", "replace"),
            HostChoice::new("extend", "extend"),
        ],
        SelectionMode::One,
    )
    .unwrap()
    .default_value(serde_json::json!("extend"))
    .default_selection(DefaultSelection::Focused);
    let request = HostInputRequest::questionnaire("Prompt", vec![question]).unwrap();
    let translated = questionnaire_request(&request);

    assert_eq!(
        translated.questions[0].default_selection,
        crate::questionnaire::QuestionnaireDefaultSelection::Focused
    );
    assert_eq!(
        translated.questions[0].default,
        Some(serde_json::json!("extend"))
    );

    let (response, display) = submit(translated, |composer| composer.toggle_active_choice());
    let host = host_response(response);

    assert_eq!(display, "extend");
    assert_eq!(host.answers()["prompt"], ["extend"]);
    assert!(request.validate(&host).is_ok());
}

#[test]
fn yes_no_round_trip_preserves_confirm_semantics_and_values() {
    let question = HostQuestion::new(
        "apply",
        "Apply changes?",
        vec![HostChoice::new("yes", "Yes"), HostChoice::new("no", "No")],
        SelectionMode::One,
    )
    .unwrap();
    let request = HostInputRequest::questionnaire("Confirm", vec![question]).unwrap();
    let translated = questionnaire_request(&request);

    assert_eq!(
        translated.questions[0].kind,
        QuestionnaireQuestionKind::Confirm
    );

    let (response, display) = submit(translated, |composer| composer.toggle_active_choice());
    let host = host_response(response);

    assert_eq!(display, "Yes");
    assert_eq!(host.answers()["apply"], ["yes"]);
    assert!(request.validate(&host).is_ok());
}

#[test]
fn optional_unanswered_round_trip_omits_the_answer() {
    let question = HostQuestion::new(
        "language",
        "Language?",
        vec![HostChoice::new("rust", "Rust")],
        SelectionMode::One,
    )
    .unwrap()
    .optional();
    let request = HostInputRequest::questionnaire("Optional", vec![question]).unwrap();
    let translated = questionnaire_request(&request);

    let (response, _display) = submit(translated, |_| {});
    let host = host_response(response);

    assert!(host.answers().is_empty());
    assert!(request.validate(&host).is_ok());
}

#[test]
fn multi_select_round_trip_renders_labels_and_returns_values() {
    let question = HostQuestion::new(
        "tests",
        "Test suites?",
        vec![
            HostChoice::new("unit_tests", "Unit tests"),
            HostChoice::new("e2e", "End to end"),
        ],
        SelectionMode::Many,
    )
    .unwrap();
    let request = HostInputRequest::questionnaire("Tests", vec![question]).unwrap();
    let translated = questionnaire_request(&request);

    let (response, display) = submit(translated, |composer| {
        composer.toggle_active_choice();
        composer.move_active_choice_next();
        composer.toggle_active_choice();
    });
    let host = host_response(response);

    assert_eq!(display, "Unit tests, End to end");
    assert_eq!(host.answers()["tests"], ["unit_tests", "e2e"]);
    assert!(request.validate(&host).is_ok());
}

fn submit(
    request: crate::tui::questionnaire::QuestionnaireRequest,
    interact: impl FnOnce(&mut QuestionnaireComposer),
) -> (QuestionnaireResponse, String) {
    let (reply_tx, mut reply_rx) = tokio::sync::oneshot::channel();
    let mut composer =
        QuestionnaireComposer::new(request, QuestionnaireResponseChannel::new(reply_tx));
    interact(&mut composer);
    let submitted = composer.submit().unwrap();
    let reply = reply_rx.try_recv().unwrap();
    let QuestionnaireReply::Answer(response) = reply else {
        panic!("expected questionnaire answer");
    };
    (response, submitted.display)
}
