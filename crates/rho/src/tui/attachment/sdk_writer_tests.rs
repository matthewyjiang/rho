use std::path::PathBuf;

use pretty_assertions::assert_eq;
use rho_tools::tool::ToolDisplayStyle;
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
    assert!(attachment_update(ViewModelEvent::SteeringApplied(Vec::new())).is_none());
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
    assert_eq!(
        started,
        vec![AttachmentEvent::ToolStarted {
            display_lines: running.to_display_lines(),
            card: Some(running),
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
        attachment_update(ViewModelEvent::ToolFinished {
            call_id: compaction_call_id(),
            ok: true,
            display_style: ToolDisplayStyle::default_tool(),
            card: card.clone(),
            image_asset: None,
        }),
        Some(AttachmentEvent::ToolFinished {
            ok: true,
            display_style: ToolDisplayStyle::default_tool(),
            display_lines: card.to_display_lines(),
            card: Some(card),
        })
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
            ok: false,
            display_style: ToolDisplayStyle::default_tool(),
            display_lines: failed.to_display_lines(),
            card: Some(failed),
        }
    );
    assert_eq!(
        events[1],
        AttachmentEvent::Failed("provider unavailable".into())
    );
}
