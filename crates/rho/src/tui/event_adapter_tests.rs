use std::time::Duration;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ModelUsage, ToolCall},
    tool::{OperationKind, ToolAsset, ToolMetadata, ToolOutput, ToolProgress},
    HostChoice, HostInputRequest, HostQuestion, ProviderStreamResetReason, Revision, RunEvent,
    RunId, SelectionMode, ToolCallId, ToolCompletion,
};
use rho_tools::tool_card::{
    DiffRow, DiffRowKind, ToolBody, ToolFact, ToolFamily, ToolHeader, ToolStatus,
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
    assert!(matches!(
        only_event(adapter.translate(RunEvent::ModelCallCompleted {
            profile: rho_sdk::ModelCallProfile {
                provider: "openai".into(),
                model: "gpt".into(),
                reasoning: rho_sdk::ReasoningLevel::Medium,
                service_tier: None,
            },
            metrics: rho_sdk::ModelCallMetrics {
                output_tokens: Some(3),
                time_to_first_token: Some(Duration::from_millis(200)),
                generation_time: Some(Duration::from_secs(1)),
                total_latency: Duration::from_millis(1_200),
            },
        })),
        ViewEvent::Update(ViewModelEvent::ModelCallCompleted { profile, metrics })
            if profile.model == "gpt" && metrics.output_tokens == Some(3)
    ));
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
            ViewEvent::Update(ViewModelEvent::ProviderStreamReset(_))
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

    let ViewEvent::Update(ViewModelEvent::ToolFinished { card, .. }) =
        only_event(adapter.translate(RunEvent::WebSearch {
            detail: "rho docs".into(),
        }))
    else {
        panic!("expected web search tool finished");
    };
    assert_eq!(card.status, ToolStatus::Ok);
    assert_eq!(card.family, ToolFamily::Web);
    assert_eq!(card.header, ToolHeader::call("web_search", None));
    assert_eq!(
        card.facts,
        vec![
            rho_tools::tool_card::ToolFact::Text {
                text: "rho docs".into(),
            },
            rho_tools::tool_card::ToolFact::Meta {
                text: "finished".into(),
            },
        ]
    );
}

#[test]
fn provider_native_hosted_tool_activity_maps_to_tool_finished_view() {
    let mut adapter = SdkEventAdapter::default();

    let ViewEvent::Update(ViewModelEvent::ToolFinished { card, .. }) =
        only_event(adapter.translate(RunEvent::HostedToolActivity {
            name: "x_search".into(),
            detail: "xAI".into(),
        }))
    else {
        panic!("expected hosted tool activity finished");
    };
    assert_eq!(card.status, ToolStatus::Ok);
    assert_eq!(card.family, ToolFamily::Web);
    assert_eq!(card.header, ToolHeader::call("x_search", None));
    assert_eq!(
        card.facts,
        vec![
            rho_tools::tool_card::ToolFact::Text { text: "xAI".into() },
            rho_tools::tool_card::ToolFact::Meta {
                text: "finished".into(),
            },
        ]
    );
}

#[test]
fn unknown_hosted_tool_activity_uses_default_family() {
    let mut adapter = SdkEventAdapter::default();

    let ViewEvent::Update(ViewModelEvent::ToolFinished { card, .. }) =
        only_event(adapter.translate(RunEvent::HostedToolActivity {
            name: "code_interpreter".into(),
            detail: "ran analysis".into(),
        }))
    else {
        panic!("expected hosted tool activity finished");
    };
    assert_eq!(card.family, ToolFamily::Default);
    assert_eq!(card.header, ToolHeader::call("code_interpreter", None));
    assert_eq!(
        card.facts,
        vec![
            rho_tools::tool_card::ToolFact::Text {
                text: "ran analysis".into(),
            },
            rho_tools::tool_card::ToolFact::Meta {
                text: "finished".into(),
            },
        ]
    );
}

#[test]
fn hosted_tool_activity_without_detail_uses_only_finished_fact() {
    let mut adapter = SdkEventAdapter::default();

    let ViewEvent::Update(ViewModelEvent::ToolFinished { card, .. }) =
        only_event(adapter.translate(RunEvent::HostedToolActivity {
            name: "x_search".into(),
            detail: String::new(),
        }))
    else {
        panic!("expected hosted tool activity finished");
    };
    assert_eq!(card.header, ToolHeader::call("x_search", None));
    assert_eq!(
        card.facts,
        vec![rho_tools::tool_card::ToolFact::Meta {
            text: "finished".into(),
        }]
    );
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
fn edit_keeps_one_diff_card_from_stream_through_completion() {
    let mut adapter = SdkEventAdapter::default();
    let call_id = ToolCallId::from_string("call-1").unwrap();
    let input = "[src/lib.rs#A1B2C3D4]\nPUT 1.=1:\n+new\n";
    let partial_arguments = r#"{"input":"[src/lib.rs#A1B2C3D4]\nPUT 1.=1:\n+new\n"#;
    let partial_events = adapter.translate(RunEvent::ToolCallUpdated {
        index: 0,
        id: None,
        name: None,
        arguments_delta: partial_arguments.into(),
    });
    // Argument-only deltas meter live usage without forcing a card event.
    assert!(matches!(
        only_event(partial_events),
        ViewEvent::Update(ViewModelEvent::LiveOutputText(text))
            if text == partial_arguments
    ));
    let ViewEvent::Update(ViewModelEvent::ToolCallUpdated { card, .. }) =
        only_event(adapter.translate(RunEvent::ToolCallUpdated {
            index: 0,
            id: None,
            name: Some("edit".into()),
            arguments_delta: String::new(),
        }))
    else {
        panic!("expected streamed tool call");
    };
    let proposed_card = rho_tools::tool_card::ToolCard::new(
        ToolStatus::Running,
        ToolFamily::FileDiff,
        ToolHeader::call("edit", Some("src/lib.rs".into())),
    )
    .with_facts(vec![ToolFact::DiffStat {
        added: 1,
        removed: 1,
        path: Some("src/lib.rs".into()),
    }])
    .with_body(ToolBody::Diff(vec![
        DiffRow::new(DiffRowKind::Removed, Some(1), ""),
        DiffRow::new(DiffRowKind::Added, None, "new"),
    ]));
    assert_eq!(card, Some(proposed_card.clone()));
    assert!(matches!(
        only_event(adapter.translate(RunEvent::ToolCallUpdated {
            index: 0,
            id: Some(call_id.to_string()),
            name: None,
            arguments_delta: String::new(),
        })),
        ViewEvent::Update(ViewModelEvent::ToolCallUpdated {
            call_id: Some(bound_id),
            card: None,
            ..
        }) if bound_id == call_id
    ));
    let mut interrupted_card = proposed_card.clone();
    interrupted_card.status = ToolStatus::Interrupted;
    assert_eq!(
        crate::app::interactive_presenter::InteractiveToolPresenter::new(
            std::path::PathBuf::new(),
        )
        .interrupted(Some("edit"), partial_arguments)
        .card,
        interrupted_card
    );

    let call = ToolCall {
        id: call_id.to_string(),
        name: "edit".into(),
        arguments: serde_json::json!({"input": input}),
    };
    assert!(matches!(
        only_event(adapter.translate(RunEvent::ToolProposed { call })),
        ViewEvent::Update(ViewModelEvent::ToolCallProposed { card, .. })
            if card == proposed_card
    ));
    assert!(matches!(
        only_event(adapter.translate(RunEvent::ToolStarted {
            call_id: call_id.clone(),
            name: "edit".into(),
            metadata: ToolMetadata::new().operation(OperationKind::Write),
        })),
        ViewEvent::Update(ViewModelEvent::ToolStarted { card, .. })
            if card == proposed_card
    ));
    let progress = ToolProgress::message("applying").units(1, 2);
    let ViewEvent::Update(ViewModelEvent::ToolUpdated { card, .. }) =
        only_event(adapter.translate(RunEvent::ToolUpdated {
            call_id: call_id.clone(),
            progress,
        }))
    else {
        panic!("expected tool progress");
    };
    assert!(
        card.body.is_diff()
            || card
                .facts
                .iter()
                .any(|fact| matches!(fact, ToolFact::DiffStat { .. }))
            || matches!(card.body, ToolBody::Lines(_))
    );
    assert!(
        card.facts.contains(&ToolFact::Meta {
            text: "applying".into(),
        }) || matches!(
            &card.body,
            ToolBody::Lines(lines) if lines.iter().any(|line| line.contains("applying"))
        ),
        "progress should surface applying text: {card:?}"
    );
    assert!(card.facts.contains(&ToolFact::Progress {
        completed: 1,
        total: Some(2),
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
    assert_eq!(card.status, ToolStatus::Ok);
    assert_eq!(
        card.header,
        ToolHeader::call("edit", Some("src/lib.rs".into()))
    );
    assert_eq!(
        card.facts,
        vec![ToolFact::DiffStat {
            added: 1,
            removed: 1,
            path: Some("src/lib.rs".into()),
        }]
    );
    assert_eq!(
        card.body,
        ToolBody::Diff(vec![
            DiffRow::new(DiffRowKind::Removed, Some(1), "old"),
            DiffRow::new(DiffRowKind::Added, Some(1), "new"),
        ])
    );
}

#[test]
fn edit_binds_a_late_call_id_after_a_large_preview_stride() {
    let mut adapter = SdkEventAdapter::default();
    let call_id = ToolCallId::from_string("call-large-preview").unwrap();
    let input = format!(
        "[large.txt#DEADBEEF]\nPUT 1.=1:\n{}",
        "+line\n".repeat(45_000)
    );
    let arguments = serde_json::to_string(&serde_json::json!({"input": input})).unwrap();
    let events = adapter.translate(RunEvent::ToolCallUpdated {
        index: 0,
        id: None,
        name: Some("edit".into()),
        arguments_delta: arguments,
    });
    assert!(events
        .iter()
        .any(|event| matches!(event, ViewEvent::Update(ViewModelEvent::LiveOutputText(_)))));
    assert!(events.iter().any(|event| matches!(
        event,
        ViewEvent::Update(ViewModelEvent::ToolCallUpdated {
            call_id: None,
            card: Some(_),
            ..
        })
    )));

    // Identity-only delta after a coarse stride: bind without re-rendering.
    assert!(matches!(
        only_event(adapter.translate(RunEvent::ToolCallUpdated {
            index: 0,
            id: Some(call_id.to_string()),
            name: None,
            arguments_delta: String::new(),
        })),
        ViewEvent::Update(ViewModelEvent::ToolCallUpdated {
            call_id: Some(bound_id),
            card: None,
            ..
        }) if bound_id == call_id
    ));
    assert!(adapter
        .translate(RunEvent::ToolCallUpdated {
            index: 0,
            id: Some(call_id.to_string()),
            name: None,
            arguments_delta: String::new(),
        })
        .is_empty());
}

// Covers: multi-file edit previews keep each path identity
// Owner: interactive presenter format
#[test]
fn edit_preview_preserves_multi_file_identity() {
    let input = "[a.txt#AAAAAAAA]\nPUT 1.=1:\n+A\n\n[b.txt#BBBBBBBB]\nCUT 1.=1\n";
    let expected = rho_tools::tool_card::ToolCard::new(
        ToolStatus::Running,
        ToolFamily::FileDiff,
        ToolHeader::call("edit", Some("2 files".into())),
    )
    .with_facts(vec![
        ToolFact::DiffStat {
            added: 1,
            removed: 1,
            path: Some("a.txt".into()),
        },
        ToolFact::DiffStat {
            added: 0,
            removed: 1,
            path: Some("b.txt".into()),
        },
    ])
    .with_body(ToolBody::Diff(vec![
        DiffRow::new(DiffRowKind::File, None, "a.txt"),
        DiffRow::new(DiffRowKind::Removed, Some(1), ""),
        DiffRow::new(DiffRowKind::Added, None, "A"),
        DiffRow::new(DiffRowKind::File, None, "b.txt"),
        DiffRow::new(DiffRowKind::Removed, Some(1), ""),
    ]));

    let mut adapter = SdkEventAdapter::default();
    let arguments = serde_json::to_string(&serde_json::json!({"input": input})).unwrap();
    let events = adapter.translate(RunEvent::ToolCallUpdated {
        index: 0,
        id: None,
        name: Some("edit".into()),
        arguments_delta: arguments,
    });
    let Some(card) = events.into_iter().find_map(|event| match event {
        ViewEvent::Update(ViewModelEvent::ToolCallUpdated {
            card: Some(card), ..
        }) => Some(card),
        _ => None,
    }) else {
        panic!("expected streamed edit card");
    };
    assert_eq!(card, expected);
}

#[test]
fn write_file_does_not_label_a_mixed_omitted_diff_as_no_changes() {
    let mut adapter = SdkEventAdapter::default();
    let call_id = ToolCallId::from_string("call-omitted-diff").unwrap();
    let _ = only_event(adapter.translate(RunEvent::ToolStarted {
        call_id: call_id.clone(),
        name: "write_file".into(),
        metadata: ToolMetadata::new().operation(OperationKind::Write),
    }));
    let output = ToolOutput::text("updated").metadata(
        ToolMetadata::new()
            .operation(OperationKind::Write)
            .affected_path("large.txt")
            .diff("No changes.\n\nDiff omitted: test reason."),
    );

    let ViewEvent::Update(ViewModelEvent::ToolFinished { card, .. }) =
        only_event(adapter.translate(RunEvent::ToolFinished {
            call_id,
            result: ToolCompletion::Success(output),
        }))
    else {
        panic!("expected translated tool completion");
    };

    assert_eq!(
        card.header,
        ToolHeader::call("write_file", Some("large.txt".into()))
    );
    assert_eq!(
        card.facts,
        vec![ToolFact::Meta {
            text: "Diff omitted: test reason.".into(),
        }]
    );
    assert!(card.body.is_empty());
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
    assert_eq!(card.status, ToolStatus::Ok);
    assert_eq!(card.header, ToolHeader::call("read_file", None));
    assert_eq!(
        card.facts,
        vec![
            ToolFact::Count {
                label: "line".into(),
                value: 1,
                detail: None,
            },
            ToolFact::Meta {
                text: "image preview unavailable: invalid image".into(),
            },
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
            && card.status == ToolStatus::Error
            && card.facts.iter().any(|fact| matches!(
                fact,
                ToolFact::Meta { text } if text == "failed"
            ))
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
            card,
            ..
        }) if call_id == &crate::tui::compaction_display::compaction_call_id()
            && card.status == ToolStatus::Interrupted
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

#[test]
fn rate_limit_stream_reset_carries_retry_after_into_view_model() {
    use std::time::Duration;

    use crate::tui::activity::ProviderRetryHint;

    let mut adapter = SdkEventAdapter::default();
    let event = only_event(adapter.translate(RunEvent::ProviderStreamReset {
        reason: ProviderStreamResetReason::retryable_failure(
            rho_sdk::ProviderErrorKind::RateLimit,
            Some(Duration::from_secs(12)),
        ),
        detail: "retrying".into(),
    }));

    assert!(matches!(
        event,
        ViewEvent::Update(ViewModelEvent::ProviderStreamReset(ProviderRetryHint {
            reason: ProviderStreamResetReason::RetryableFailureWithRetryAfter {
                kind: rho_sdk::ProviderErrorKind::RateLimit,
                retry_after: delay,
            },
        })) if delay == Duration::from_secs(12)
    ));
}
