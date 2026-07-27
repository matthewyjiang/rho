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

fn only_event(events: Vec<ViewEvent>) -> ViewEvent {
    assert_eq!(
        events.len(),
        1,
        "expected exactly one view event: {events:?}"
    );
    events.into_iter().next().expect("one event")
}

fn card_lines(card: &rho_tools::tool_card::ToolCard) -> Vec<String> {
    let mut lines = vec![card.header_text()];
    let fact_count = card.facts.len();
    for (index, fact) in card.facts.iter().enumerate() {
        let branch = if index + 1 == fact_count && card.body.is_empty() {
            "└"
        } else {
            "├"
        };
        lines.push(format!("  {branch} {}", fact.plain_text()));
    }
    let body = card.body.plain_lines();
    for (index, line) in body.iter().enumerate() {
        if index == 0 && card.facts.is_empty() && body.len() == 1 && !line.contains('\n') {
            lines.push(format!("  └ {line}"));
        } else if index == 0 {
            lines.push(String::new());
            lines.push(line.clone());
        } else {
            lines.push(line.clone());
        }
    }
    lines
}

#[test]
fn translates_streaming_and_usage_events_without_rendering_state() {
    let mut adapter = SdkEventAdapter::default();

    assert!(matches!(
        only_event(adapter.translate(RunEvent::Started {
            run_id: RunId::new(),
            revision: Revision::INITIAL,
        })),
        ViewEvent::Update(ViewModelEvent::RunStarted)
    ));
    assert!(matches!(
        only_event(adapter.translate(RunEvent::AssistantTextDelta {
            text: "hello".into()
        })),
        ViewEvent::Update(ViewModelEvent::OutputDelta(text)) if text == "hello"
    ));
    let usage = ModelUsage {
        output_tokens: Some(3),
        ..ModelUsage::default()
    };
    assert!(matches!(
        only_event(adapter.translate(RunEvent::UsageUpdated {
            usage: usage.clone()
        })),
        ViewEvent::Update(ViewModelEvent::Usage(translated)) if translated == usage
    ));
}

#[test]
fn provider_diagnostics_are_shown_in_interactive_failures() {
    let mut adapter = SdkEventAdapter::default();

    let event = only_event(adapter.translate(RunEvent::ProviderDiagnostic {
        detail: rho_sdk::ProviderDiagnostic::new("{\"error\":\"bad request\"}"),
    }));

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
            only_event(adapter.translate(RunEvent::ProviderStreamReset {
                reason,
                detail: "retrying".into(),
            })),
            ViewEvent::Update(ViewModelEvent::ProviderStreamReset)
        ));
    }
}

#[test]
fn physical_provider_retry_maps_to_typed_view_model_event() {
    let mut adapter = SdkEventAdapter::default();

    assert!(matches!(
        only_event(adapter.translate(RunEvent::ProviderRequestRetry)),
        ViewEvent::Update(ViewModelEvent::ProviderRetry)
    ));
}

#[test]
fn provider_native_web_search_maps_to_tool_finished_view() {
    let mut adapter = SdkEventAdapter::default();

    assert!(matches!(
        only_event(adapter.translate(RunEvent::WebSearch {
            detail: "rho docs".into(),
        })),
        ViewEvent::Update(ViewModelEvent::ToolFinished {
            ref card,
            ..
        }) if card_lines(card) == ["✓ web_search(rho docs)"]
    ));
}

#[test]
fn legacy_provider_activity_is_ignored_by_tui() {
    let mut adapter = SdkEventAdapter::default();

    #[allow(deprecated)]
    let events = adapter.translate(RunEvent::ProviderActivity {
        kind: rho_sdk::PROVIDER_ACTIVITY_WEB_SEARCH.into(),
        detail: "rho docs".into(),
    });
    assert!(events.is_empty());
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
    let _ = only_event(adapter.translate(RunEvent::ToolProposed { call }));
    let _ = only_event(adapter.translate(RunEvent::ToolStarted {
        call_id: call_id.clone(),
        name: "edit_file".into(),
        metadata: ToolMetadata::new().operation(OperationKind::Write),
    }));
    let output = ToolOutput::text("updated").metadata(
        ToolMetadata::new()
            .affected_path("src/lib.rs")
            .diff("--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n"),
    );

    let ViewEvent::Update(ViewModelEvent::ToolFinished {
        call_id: translated_call_id,
        card,
        ..
    }) = only_event(adapter.translate(RunEvent::ToolFinished {
        call_id: call_id.clone(),
        result: ToolCompletion::Success(output),
    }))
    else {
        panic!("expected translated tool completion");
    };

    assert_eq!(translated_call_id, call_id);
    assert_eq!(
        card_lines(&card),
        vec![
            "✓ edit_file(src/lib.rs)".to_string(),
            "  ├ +1 -1 lines | src/lib.rs".to_string(),
            String::new(),
            "-old".to_string(),
            "+new".to_string(),
        ]
    );
}

#[test]
fn forwards_image_asset_on_tool_completion() {
    let mut adapter = SdkEventAdapter::default();
    let call_id = ToolCallId::from_string("call-image").unwrap();
    let _ = only_event(adapter.translate(RunEvent::ToolStarted {
        call_id: call_id.clone(),
        name: "read_file".into(),
        metadata: ToolMetadata::new(),
    }));
    let asset = ToolAsset::new("image/png", vec![1, 2, 3, 4]);
    let output = ToolOutput::text("image/png image (4 bytes)").metadata(
        ToolMetadata::new()
            .asset(asset.clone())
            .presentation_notice("image preview unavailable: invalid image"),
    );

    let ViewEvent::Update(ViewModelEvent::ToolFinished {
        call_id: translated_call_id,
        image_asset,
        card,
        ..
    }) = only_event(adapter.translate(RunEvent::ToolFinished {
        call_id: call_id.clone(),
        result: ToolCompletion::Success(output),
    }))
    else {
        panic!("expected translated tool completion");
    };

    assert_eq!(translated_call_id, call_id);
    assert_eq!(image_asset, Some(asset));
    assert_eq!(
        card_lines(&card),
        [
            "✓ read_file".to_string(),
            "  ├ 1 line".to_string(),
            "  └ image preview unavailable: invalid image".to_string(),
        ]
    );
}

#[test]
fn compaction_failure_closes_open_tool_block_before_run_failed() {
    let mut adapter = SdkEventAdapter::default();
    assert!(matches!(
        only_event(adapter.translate(RunEvent::CompactionStarted {
            trigger: rho_sdk::CompactionTrigger::Automatic,
            message_count: 3,
        })),
        ViewEvent::Update(ViewModelEvent::ToolStarted { call_id, card, .. })
            if call_id == crate::tui::compaction_display::compaction_call_id()
                && card == crate::tui::compaction_display::running_card()
    ));

    let events = adapter.translate(RunEvent::Failed {
        message: "provider unavailable".into(),
        retryability: rho_sdk::Retryability::Retryable,
    });
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        ViewEvent::Update(ViewModelEvent::ToolFinished {
            call_id,
            card,
            ..
        }) if call_id == &crate::tui::compaction_display::compaction_call_id()
            && card_lines(card).iter().any(|line| line.contains("failed"))
    ));
    assert!(matches!(
        &events[1],
        ViewEvent::Failed(message) if message == "provider unavailable"
    ));
}

#[test]
fn compaction_cancel_closes_open_tool_block_before_run_cancelled() {
    let mut adapter = SdkEventAdapter::default();
    let _ = only_event(adapter.translate(RunEvent::CompactionStarted {
        trigger: rho_sdk::CompactionTrigger::Automatic,
        message_count: 1,
    }));
    let events = adapter.translate(RunEvent::Cancelled {
        revision: Revision::INITIAL,
    });
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        ViewEvent::Update(ViewModelEvent::ToolFinished {
            call_id,
            ..
        }) if call_id == &crate::tui::compaction_display::compaction_call_id()
    ));
    assert!(matches!(&events[1], ViewEvent::Cancelled));
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
