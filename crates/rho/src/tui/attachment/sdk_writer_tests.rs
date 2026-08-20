use std::{path::PathBuf, time::Duration};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::{
    run_artifacts::{AttachmentEvent, AttachmentReader},
    subagent,
    tui::compaction_display::{
        compaction_call_id, completed_card, running_card, CompactionDisplayFacts,
        CompactionUiOutcome,
    },
};

#[test]
fn attachment_stream_round_trips_view_events() {
    let directory = TempDir::new().unwrap();
    let result_path = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut writer = SdkAttachmentWriter::new(
        &result_path,
        PathBuf::from("/workspace"),
        "inspect the code",
    )
    .unwrap();
    writer
        .on_event(&rho_sdk::RunEvent::AssistantTextDelta {
            text: "found it".into(),
        })
        .unwrap();
    drop(writer);

    let mut reader = AttachmentReader::new(directory.path().join(subagent::ATTACHMENT_FILE_NAME));
    let events = reader.read_new().unwrap();

    assert_eq!(
        events,
        vec![
            AttachmentEvent::Prompt("inspect the code".into()),
            AttachmentEvent::AssistantTextDelta("found it".into()),
        ]
    );
    assert!(reader.read_new().unwrap().is_empty());
}

#[test]
fn attachment_stream_ignores_steering_applied() {
    let mut adapter = SdkEventAdapter::default();
    assert!(attachment_update(&mut adapter, ViewModelEvent::SteeringApplied(Vec::new())).is_none());
}

#[test]
fn compaction_run_events_project_to_tool_attachment_blocks() {
    let mut adapter = SdkEventAdapter::new(PathBuf::from("/workspace"));

    let started = translate_run_event(
        &mut adapter,
        &rho_sdk::RunEvent::CompactionStarted {
            trigger: rho_sdk::CompactionTrigger::Automatic,
            message_count: 3,
        },
    );
    let running = running_card();
    let key = Some(compaction_call_id().to_string());
    assert_eq!(
        started,
        vec![AttachmentEvent::ToolStarted {
            key: key.clone(),
            card: running
        }]
    );

    // Completion is already tool-shaped at the view-model boundary; attach only
    // projects ToolFinished structurally.
    let facts = CompactionDisplayFacts {
        previous_messages: 12,
        current_messages: 4,
        previous_tokens: 12_400,
        current_tokens: 3_100,
        cost_usd_micros: None,
    };
    let card = completed_card(facts);
    assert_eq!(
        attachment_update(
            &mut adapter,
            ViewModelEvent::ToolFinished {
                call_id: compaction_call_id(),
                card: card.clone(),
                image_asset: None,
            }
        ),
        Some(AttachmentEvent::ToolFinished { key, card })
    );
}

#[test]
fn open_compaction_failure_emits_tool_finish_then_failed() {
    let mut adapter = SdkEventAdapter::new(PathBuf::from("/workspace"));
    let _ = translate_run_event(
        &mut adapter,
        &rho_sdk::RunEvent::CompactionStarted {
            trigger: rho_sdk::CompactionTrigger::Automatic,
            message_count: 1,
        },
    );

    let events = translate_run_event(
        &mut adapter,
        &rho_sdk::RunEvent::Failed {
            message: "provider unavailable".into(),
            retryability: rho_sdk::Retryability::Retryable,
        },
    );
    assert_eq!(events.len(), 2);
    let failed = CompactionUiOutcome::Failed {
        detail: "provider unavailable".into(),
    }
    .card();
    assert_eq!(
        events[0],
        AttachmentEvent::ToolFinished {
            key: Some(compaction_call_id().to_string()),
            card: failed
        }
    );
    assert_eq!(
        events[1],
        AttachmentEvent::Failed("provider unavailable".into())
    );
}

#[test]
fn call_id_less_preview_and_later_update_reuse_the_same_key() {
    use rho_tools::tool_card::{ToolCard, ToolFamily, ToolHeader, ToolStatus};

    let mut adapter = SdkEventAdapter::default();
    let preview = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::Default,
        ToolHeader::call("read_file", /*primary*/ None),
    );
    let with_id = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::Default,
        ToolHeader::call("read_file", /*primary*/ Some("src/main.rs".into())),
    );
    let call_id = rho_sdk::ToolCallId::from_string("call-stable").unwrap();

    let first = attachment_update(
        &mut adapter,
        ViewModelEvent::ToolCallUpdated {
            index: 0,
            call_id: None,
            card: Some(preview.clone()),
        },
    );
    let second = attachment_update(
        &mut adapter,
        ViewModelEvent::ToolCallUpdated {
            index: 0,
            call_id: Some(call_id.clone()),
            card: Some(with_id.clone()),
        },
    );
    let finished = attachment_update(
        &mut adapter,
        ViewModelEvent::ToolFinished {
            call_id,
            card: with_id.clone(),
            image_asset: None,
        },
    );

    assert_eq!(
        first,
        Some(AttachmentEvent::ToolStarted {
            key: Some("preview:0".into()),
            card: preview,
        })
    );
    assert_eq!(
        second,
        Some(AttachmentEvent::ToolStarted {
            key: Some("preview:0".into()),
            card: with_id.clone(),
        })
    );
    assert_eq!(
        finished,
        Some(AttachmentEvent::ToolFinished {
            key: Some("preview:0".into()),
            card: with_id,
        })
    );
}

fn model_call_completed(
    output_tokens: Option<u64>,
    generation_time: Option<Duration>,
) -> rho_sdk::RunEvent {
    rho_sdk::RunEvent::ModelCallCompleted {
        profile: rho_sdk::ModelCallProfile {
            provider: "openai".into(),
            model: "gpt".into(),
            reasoning: rho_sdk::ReasoningLevel::Medium,
            service_tier: None,
        },
        metrics: rho_sdk::ModelCallMetrics {
            output_tokens,
            time_to_first_token: Some(Duration::from_millis(200)),
            generation_time,
            total_latency: Duration::from_millis(2_200),
        },
    }
}

// Covers: attach journals resolved generation tokens and time, including the
// 1.x ProviderActivity carrier path.
// Owner: attach SDK writer
#[test]
fn model_call_completed_journals_resolved_tokens_and_time() {
    let mut adapter = SdkEventAdapter::default();
    assert_eq!(
        translate_run_event(
            &mut adapter,
            &model_call_completed(Some(100), Some(Duration::from_secs(2))),
        ),
        vec![AttachmentEvent::ModelCallCompleted {
            generation_output_tokens: 100,
            generation_time_ms: 2_000,
        }]
    );

    #[allow(deprecated)]
    let carrier = rho_sdk::RunEvent::ProviderActivity {
        kind: "model_call_generation_output_tokens".into(),
        detail: "80".into(),
    };
    assert!(translate_run_event(&mut adapter, &carrier).is_empty());
    assert_eq!(
        translate_run_event(
            &mut adapter,
            &model_call_completed(Some(100), Some(Duration::from_secs(2))),
        ),
        vec![AttachmentEvent::ModelCallCompleted {
            generation_output_tokens: 80,
            generation_time_ms: 2_000,
        }]
    );
}

// Covers: attach omits model-call lines without both resolved tokens and time.
// Owner: attach SDK writer
#[test]
fn model_call_completed_without_tokens_or_time_is_not_journaled() {
    let mut adapter = SdkEventAdapter::default();
    #[allow(deprecated)]
    let unavailable = rho_sdk::RunEvent::ProviderActivity {
        kind: "model_call_generation_output_tokens".into(),
        detail: "unavailable".into(),
    };
    assert!(translate_run_event(&mut adapter, &unavailable).is_empty());
    assert!(translate_run_event(
        &mut adapter,
        &model_call_completed(Some(100), Some(Duration::from_secs(2))),
    )
    .is_empty());
    assert!(translate_run_event(&mut adapter, &model_call_completed(Some(100), None),).is_empty());
}
