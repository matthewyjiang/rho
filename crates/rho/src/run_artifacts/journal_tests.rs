use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

// Covers: old card-only journals remain readable while message rows persist
// only their real presentation. Owner: attachment journal wire contract.
#[test]
fn finished_presentations_preserve_journal_shape() {
    use crate::presentation::{NotificationCard, NotificationDelivery, Presentation};
    use rho_tools::tool_card::{ToolCard, ToolFamily, ToolHeader, ToolStatus};

    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Default,
        ToolHeader::call("read_file", None),
    );
    let message = Box::new(NotificationCard {
        title: "Inspect routing".into(),
        sender: "parent".into(),
        recipient: "reviewer".into(),
        delivery: NotificationDelivery::Queued,
        tone: crate::presentation::NotificationTone::Neutral,
        preview: crate::presentation::NotificationPreview::Truncated,
        visibility: crate::presentation::NotificationVisibility::Activity,
        reference: None,
        subtitle: None,
        body: "Check the queued route.".into(),
        output: None,
        details: vec!["run: abc123".into()],
    });
    for (presentation, data) in [
        (
            Presentation::Card(card.clone().into()),
            serde_json::json!({"card": card}),
        ),
        (
            Presentation::Notification(message.clone()),
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
