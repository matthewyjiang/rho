use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

// Covers: old card-only journals remain readable while message rows persist
// only their real presentation. Owner: attachment journal wire contract.
#[test]
fn finished_presentations_preserve_journal_shape() {
    use crate::presentation::{MessageCard, MessageDelivery, Presentation};
    use rho_tools::tool_card::{ToolFamily, ToolHeader, ToolStatus};

    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Default,
        ToolHeader::call("read_file", None),
    );
    let message = Box::new(MessageCard {
        title: "Inspect routing".into(),
        sender: "parent".into(),
        recipient: "reviewer".into(),
        delivery: MessageDelivery::Queued,
        body: "Check the queued route.".into(),
        details: vec!["run: abc123".into()],
    });
    for (presentation, data) in [
        (
            Presentation::Card(card.clone()),
            serde_json::json!({"card": card}),
        ),
        (
            Presentation::Message(message.clone()),
            serde_json::json!({"message": message}),
        ),
    ] {
        let wire = serde_json::json!({"type": "tool_finished", "data": data});
        let event = AttachmentEvent::ToolFinished {
            key: None,
            presentation,
        };
        assert_eq!(
            serde_json::from_value::<AttachmentEvent>(wire.clone()).unwrap(),
            event
        );
        assert_eq!(serde_json::to_value(event).unwrap(), wire);
    }
}

#[test]
fn attachment_stream_skips_malformed_events() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join(subagent::ATTACHMENT_FILE_NAME);
    std::fs::write(
        &path,
        concat!(
            "not json\n",
            "{\"type\":\"assistant_text_delta\",\"data\":\"valid\"}\n"
        ),
    )
    .unwrap();
    let mut reader = AttachmentReader::new(path);

    let events = reader.read_new().unwrap();

    assert_eq!(events.len(), 2);
    match &events[0] {
        AttachmentEvent::Notice(message) => {
            assert!(
                message.contains("skipped invalid attachment event"),
                "{message}"
            );
        }
        other => panic!("expected notice for malformed event, got {other:?}"),
    }
    assert_eq!(
        events[1],
        AttachmentEvent::AssistantTextDelta("valid".into())
    );
}

// Covers: attach journals persist the tagged model-call wire shape for readers.
// Owner: run artifact journal
#[test]
fn model_call_completed_round_trips_tagged_data() {
    let event = AttachmentEvent::ModelCallCompleted {
        generation_output_tokens: 80,
        generation_time_ms: 2_000,
    };
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        serde_json::json!({
            "type": "model_call_completed",
            "data": {
                "generation_output_tokens": 80,
                "generation_time_ms": 2000
            }
        })
    );

    let directory = TempDir::new().unwrap();
    let result_path = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut writer = AttachmentWriter::create(&result_path).unwrap();
    writer.write_event(&event).unwrap();
    drop(writer);

    let mut reader = AttachmentReader::new(directory.path().join(subagent::ATTACHMENT_FILE_NAME));
    assert_eq!(reader.read_new().unwrap(), vec![event]);
}
