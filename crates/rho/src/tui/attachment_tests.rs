use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use ratatui::{backend::TestBackend, Terminal};
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

#[test]
fn provider_retry_replaces_the_failed_attempt_stream() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        HerdrReporter::default(),
    );
    app.apply_event(AttachmentEvent::Prompt("delegated task".into()));
    app.apply_event(AttachmentEvent::StepStarted);
    app.apply_event(AttachmentEvent::AssistantTextDelta("discard me".into()));
    app.apply_event(AttachmentEvent::ProviderStreamReset);
    app.apply_event(AttachmentEvent::AssistantTextDelta("keep me".into()));

    assert!(matches!(
        app.transcript.as_slice(),
        [Entry::User(prompt), Entry::Assistant(answer)]
            if prompt == "delegated task" && answer == "keep me"
    ));
}

#[test]
fn attached_view_ignores_prompt_input() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        HerdrReporter::default(),
    );
    app.apply_event(AttachmentEvent::Prompt("delegated task".into()));

    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Char('x'),
        KeyModifiers::NONE,
    )));
    app.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));

    assert_eq!(app.transcript.len(), 1);
    assert!(!app.should_quit);
    assert!(matches!(
        &app.transcript[0],
        Entry::User(prompt) if prompt == "delegated task"
    ));
}

#[test]
fn attached_view_renders_transcript_without_a_composer() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        HerdrReporter::default(),
    );
    app.status = Some(RunStatus {
        state: RunState::Running,
        preset: Some("explorer".into()),
        last_activity: Some("tool: read_file".into()),
        ..RunStatus::default()
    });
    app.apply_event(AttachmentEvent::Prompt("delegated task".into()));
    app.apply_event(AttachmentEvent::AssistantTextDelta(
        "watchable answer".into(),
    ));
    app.apply_event(AttachmentEvent::ContextUsage(ContextUsage::estimated(
        123,
        Some(456),
    )));
    app.apply_event(AttachmentEvent::Usage(ModelUsage {
        input_tokens: Some(10),
        output_tokens: Some(5),
        ..ModelUsage::default()
    }));
    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();

    terminal.draw(|frame| app.draw(frame)).unwrap();

    let screen = terminal.backend().to_string();
    assert!(screen.contains("attached to abc123"));
    assert!(screen.contains("delegated task"));
    assert!(screen.contains("watchable answer"));
    assert!(screen.contains("context 123/456"));
    assert!(screen.contains("step tokens 10/5"));
    assert!(screen.contains("read-only"));
    assert!(!screen.contains("Type a message"));
}

#[test]
fn herdr_state_follows_attached_subagent_state() {
    let status = |state| RunStatus {
        state,
        last_activity: Some("working".into()),
        ..RunStatus::default()
    };

    assert_eq!(
        herdr_status("abc123", &status(RunState::Starting)).0,
        HerdrState::Working
    );
    assert_eq!(
        herdr_status("abc123", &status(RunState::Running)).0,
        HerdrState::Working
    );
    assert_eq!(
        herdr_status("abc123", &status(RunState::Ok)).0,
        HerdrState::Idle
    );
    assert_eq!(
        herdr_status("abc123", &status(RunState::Stopped)).0,
        HerdrState::Idle
    );
    assert_eq!(
        herdr_status("abc123", &status(RunState::Error)).0,
        HerdrState::Blocked
    );
}
