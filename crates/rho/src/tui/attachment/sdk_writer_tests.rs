use std::path::PathBuf;

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
            arguments_delta: String::new(),
        },
    );
    let second = attachment_update(
        &mut adapter,
        ViewModelEvent::ToolCallUpdated {
            index: 0,
            call_id: Some(call_id.clone()),
            card: Some(with_id.clone()),
            arguments_delta: String::new(),
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
