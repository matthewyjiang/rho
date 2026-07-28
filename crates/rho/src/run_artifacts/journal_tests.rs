use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;

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
