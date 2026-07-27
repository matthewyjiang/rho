use pretty_assertions::assert_eq;
use tempfile::TempDir;

use rho_tools::tool_card::{ToolBody, ToolCard, ToolFamily, ToolHeader, ToolStatus};

use super::*;

#[test]
fn attachment_stream_writes_and_reads_events() {
    let directory = TempDir::new().unwrap();
    let result_path = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut writer = AttachmentWriter::create(&result_path).unwrap();
    writer
        .write_event(&AttachmentEvent::Prompt("inspect the code".into()))
        .unwrap();
    writer
        .write_event(&AttachmentEvent::AssistantTextDelta("found it".into()))
        .unwrap();
    let card = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::Default,
        ToolHeader::call("bash", None),
    )
    .with_body(ToolBody::Lines(vec!["cargo test".into()]));
    writer
        .write_event(&AttachmentEvent::ToolStarted { card: card.clone() })
        .unwrap();
    drop(writer);

    let mut reader = AttachmentReader::new(directory.path().join(subagent::ATTACHMENT_FILE_NAME));
    let events = reader.read_new().unwrap();

    assert_eq!(
        events,
        vec![
            AttachmentEvent::Prompt("inspect the code".into()),
            AttachmentEvent::AssistantTextDelta("found it".into()),
            AttachmentEvent::ToolStarted { card },
        ]
    );
    assert!(reader.read_new().unwrap().is_empty());
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
