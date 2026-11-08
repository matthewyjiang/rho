use std::time::Duration;

use pretty_assertions::assert_eq;
use rho_protocol::display::{ToolCardStatus, ToolFact, ToolFamily, ToolHeader};
use rho_sdk::{
    tool::{OperationKind, ToolAsset, ToolMetadata, ToolOutput},
    ProviderStreamResetReason, Revision, RunEvent, ToolCallId, ToolCompletion,
};

use super::{SdkEventAdapter, ViewEvent, ViewModelEvent};

fn only_event(events: Vec<ViewEvent>) -> ViewEvent {
    assert_eq!(
        events.len(),
        1,
        "expected exactly one view event: {events:?}"
    );
    events.into_iter().next().expect("one event")
}

#[test]
fn provider_stream_reset_carries_reason_into_view_model() {
    use crate::tui::activity::ProviderRetryHint;

    let mut adapter = SdkEventAdapter::default();

    for (case, reason) in [
        (
            "invalid response",
            ProviderStreamResetReason::InvalidResponse,
        ),
        (
            "retryable failure",
            ProviderStreamResetReason::RetryableFailure {
                kind: rho_sdk::ProviderErrorKind::Unavailable,
                retry_after: None,
            },
        ),
        (
            "rate limit with retry-after",
            ProviderStreamResetReason::RetryableFailure {
                kind: rho_sdk::ProviderErrorKind::RateLimit,
                retry_after: Some(Duration::from_secs(12)),
            },
        ),
    ] {
        let event = only_event(adapter.translate(RunEvent::ProviderStreamReset {
            reason,
            detail: "retrying".into(),
        }));
        assert!(
            matches!(
                event,
                ViewEvent::Update(ViewModelEvent::ProviderStreamReset(hint))
                    if hint == ProviderRetryHint { reason }
            ),
            "{case}"
        );
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

    let ViewEvent::Update(ViewModelEvent::ToolFinished {
        presentation: rho_protocol::display::Presentation::Card(card),
        ..
    }) = only_event(adapter.translate(RunEvent::WebSearch {
        detail: "rho docs".into(),
    }))
    else {
        panic!("expected web search tool finished");
    };
    assert_eq!(card.status, ToolCardStatus::Ok);
    assert_eq!(card.family, ToolFamily::Web);
    assert_eq!(card.header, ToolHeader::call("web_search", None));
    assert_eq!(
        card.facts,
        vec![
            rho_protocol::display::ToolFact::Text {
                text: "rho docs".into(),
            },
            rho_protocol::display::ToolFact::Meta {
                text: "finished".into(),
            },
        ]
    );
}

#[test]
fn hosted_tool_activity_maps_to_tool_finished_view() {
    let finished = ToolFact::Meta {
        text: "finished".into(),
    };
    for (case, name, detail, family, facts) in [
        (
            "known web tool",
            "x_search",
            "xAI",
            ToolFamily::Web,
            vec![ToolFact::Text { text: "xAI".into() }, finished.clone()],
        ),
        (
            "unknown tool uses default family",
            "code_interpreter",
            "ran analysis",
            ToolFamily::Default,
            vec![
                ToolFact::Text {
                    text: "ran analysis".into(),
                },
                finished.clone(),
            ],
        ),
        (
            "empty detail keeps only finished fact",
            "x_search",
            "",
            ToolFamily::Web,
            vec![finished.clone()],
        ),
    ] {
        let mut adapter = SdkEventAdapter::default();
        let ViewEvent::Update(ViewModelEvent::ToolFinished {
            presentation: rho_protocol::display::Presentation::Card(card),
            ..
        }) = only_event(adapter.translate(RunEvent::HostedToolActivity {
            name: name.into(),
            detail: detail.into(),
        }))
        else {
            panic!("{case}: expected hosted tool activity finished");
        };
        assert_eq!(
            (card.status, card.family, card.header, card.facts),
            (
                ToolCardStatus::Ok,
                family,
                ToolHeader::call(name, None),
                facts
            ),
            "{case}"
        );
    }
}

#[test]
fn edit_binds_a_late_call_id_after_a_large_preview_stride() {
    let mut adapter = SdkEventAdapter::default();
    let call_id = ToolCallId::from_string("call-large-preview").unwrap();
    let input = format!(
        "*** Begin Patch\n*** Add File: large.txt\n{}*** End Patch\n",
        "+line\n".repeat(45_000)
    );
    let arguments = serde_json::to_string(&serde_json::json!({"input": input})).unwrap();
    let events = adapter.translate(RunEvent::ToolCallUpdated {
        index: 0,
        id: None,
        name: Some("apply_patch".into()),
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

#[test]
fn write_does_not_label_a_mixed_omitted_diff_as_no_changes() {
    let mut adapter = SdkEventAdapter::default();
    let call_id = ToolCallId::from_string("call-omitted-diff").unwrap();
    let _ = only_event(adapter.translate(RunEvent::ToolStarted {
        call_id: call_id.clone(),
        name: "write".into(),
        metadata: ToolMetadata::new().operation(OperationKind::Write),
        execution: rho_sdk::ToolExecution::Attached,
    }));
    let output = ToolOutput::text("updated").metadata(
        ToolMetadata::new()
            .operation(OperationKind::Write)
            .affected_path("large.txt")
            .diff("No changes.\n\nDiff omitted: test reason."),
    );

    let ViewEvent::Update(ViewModelEvent::ToolFinished {
        presentation: rho_protocol::display::Presentation::Card(card),
        ..
    }) = only_event(adapter.translate(RunEvent::ToolFinished {
        call_id,
        result: ToolCompletion::Completed(output),
    }))
    else {
        panic!("expected translated tool completion");
    };

    assert_eq!(
        card.header,
        ToolHeader::call("write", Some("large.txt".into()))
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
        execution: rho_sdk::ToolExecution::Attached,
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
        presentation: rho_protocol::display::Presentation::Card(card),
        ..
    }) = only_event(adapter.translate(RunEvent::ToolFinished {
        call_id: call_id.clone(),
        result: ToolCompletion::Completed(output),
    }))
    else {
        panic!("expected translated tool completion");
    };

    assert_eq!(translated_call_id, call_id);
    assert_eq!(image_asset, Some(asset));
    assert_eq!(card.status, ToolCardStatus::Ok);
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
        revision: Revision::INITIAL,
    });
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        ViewEvent::Update(ViewModelEvent::ToolFinished {
            call_id,
            presentation: rho_protocol::display::Presentation::Card(card),
            ..
        }) if call_id == &crate::tui::compaction_display::compaction_call_id()
            && card.status == ToolCardStatus::Error
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
            presentation: rho_protocol::display::Presentation::Card(card),
            ..
        }) if call_id == &crate::tui::compaction_display::compaction_call_id()
            && card.status == ToolCardStatus::Interrupted
    ));
    assert!(matches!(&events[1], ViewEvent::Cancelled));
}
