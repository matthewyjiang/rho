use std::path::PathBuf;

use tempfile::TempDir;

use super::*;

#[test]
fn attachment_stream_round_trips_view_events() {
    let directory = TempDir::new().unwrap();
    let result_path = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut writer = AttachmentWriter::new(
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
fn attachment_stream_ignores_pending_input_acknowledgements() {
    assert!(attachment_update(ViewModelEvent::SteeringApplied(Vec::new())).is_none());
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

    assert!(matches!(
        &events[0],
        AttachmentEvent::Notice(message) if message.contains("skipped invalid attachment event")
    ));
    assert!(matches!(
        &events[1],
        AttachmentEvent::AssistantTextDelta(text) if text == "valid"
    ));
}
