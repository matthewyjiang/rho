use std::path::PathBuf;

use tempfile::TempDir;

use super::*;
use crate::{run_artifacts::AttachmentReader, subagent};

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

    assert!(matches!(
        &events[0],
        AttachmentEvent::Prompt(prompt) if prompt == "inspect the code"
    ));
    assert!(matches!(
        &events[1],
        AttachmentEvent::AssistantTextDelta(text) if text == "found it"
    ));
    assert!(reader.read_new().unwrap().is_empty());
}

#[test]
fn attachment_stream_ignores_steering_applied() {
    assert!(attachment_update(ViewModelEvent::SteeringApplied(Vec::new())).is_none());
}

#[test]
fn attachment_stream_preserves_compaction_tool_blocks() {
    assert!(matches!(
        attachment_update(ViewModelEvent::CompactionStarted),
        Some(AttachmentEvent::ToolStarted { display_lines })
            if display_lines == ["compact".to_string(), "shrinking context…".to_string()]
    ));
    assert!(matches!(
        attachment_update(ViewModelEvent::CompactionFinished {
            outcome: super::super::super::compaction_display::CompactionUiOutcome::Completed(
                super::super::super::compaction_display::CompactionDisplayFacts {
                    previous_messages: 12,
                    current_messages: 4,
                    previous_tokens: 12_400,
                    current_tokens: 3_100,
                    cost_usd_micros: None,
                },
            ),
        }),
        Some(AttachmentEvent::ToolFinished {
            ok: true,
            display_lines,
            ..
        }) if display_lines.iter().any(|line| line.contains("12.4K → 3.1K tokens"))
            && display_lines.iter().any(|line| line.contains("12 → 4 messages"))
    ));
    assert!(matches!(
        attachment_update(ViewModelEvent::CompactionFinished {
            outcome: super::super::super::compaction_display::CompactionUiOutcome::Failed {
                detail: "boom".into(),
            },
        }),
        Some(AttachmentEvent::ToolFinished {
            ok: false,
            display_lines,
            ..
        }) if display_lines.iter().any(|line| line == "failed")
    ));
}
