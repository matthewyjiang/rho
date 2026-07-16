use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use pretty_assertions::assert_eq;
use ratatui::{backend::TestBackend, Terminal};
use tempfile::TempDir;

use super::*;

#[test]
fn provider_retry_replaces_output_but_preserves_presented_events() {
    let directory = TempDir::new().unwrap();
    let mut app = AttachmentApp::new(
        "abc123",
        directory.path().to_path_buf(),
        HerdrReporter::default(),
    );
    app.apply_event(AttachmentEvent::Prompt("delegated task".into()));
    app.apply_event(AttachmentEvent::StepStarted);
    app.apply_event(AttachmentEvent::AssistantTextDelta("discard me".into()));
    app.apply_event(AttachmentEvent::Notice("keep notice".into()));
    app.apply_event(AttachmentEvent::ToolFinished {
        ok: true,
        display_style: crate::tool::ToolDisplayStyle::default_tool(),
        display_lines: vec!["keep tool".into()],
    });
    app.apply_event(AttachmentEvent::ReasoningDelta("discard reasoning".into()));
    app.apply_event(AttachmentEvent::ProviderStreamReset);
    app.apply_event(AttachmentEvent::AssistantTextDelta("keep me".into()));

    assert!(matches!(
        app.transcript.as_slice(),
        [
            Entry::User(prompt),
            Entry::Notice(notice),
            Entry::Tool(tool),
            Entry::Assistant(answer)
        ] if prompt == "delegated task"
            && notice == "keep notice"
            && tool.display_lines == ["keep tool"]
            && answer == "keep me"
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
