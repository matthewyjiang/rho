use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::watch;

use super::{RuntimeLabel, StatusSink};
use crate::cli_runtime::stream_effect::{
    StatusPatch, StreamEffect, TerminalClassification, TerminalResult,
};

use crate::{
    run_artifacts::{AttachmentEvent, AttachmentReader, RunArtifactIdentity},
    subagent::{self, RunState, RunStatus},
};

const TEST_LABEL: RuntimeLabel = RuntimeLabel {
    starting_activity: "starting claude",
    program: "claude code",
    resume_command: "claude",
    session_label: "claude session",
    cost_label: "claude cost",
};

fn identity() -> RunArtifactIdentity {
    RunArtifactIdentity {
        agent_id: "planner".into(),
        agent_fingerprint: "fp".into(),
        provider: "claude-code".into(),
        model: Some("opus".into()),
        runtime: crate::agent::AgentRuntime::ClaudeCli,
        reasoning: None,
    }
}

fn success_terminal() -> TerminalResult {
    TerminalResult {
        classification: TerminalClassification::Success {
            subtype: "success".into(),
        },
        result_text: Some("done".into()),
        error: None,
        session_id: Some("sess".into()),
        num_turns: Some(2),
        usage: None,
        context: None,
        total_cost_usd: Some(0.12),
        permission_denials: Vec::new(),
        stop_reason: None,
    }
}

fn read_attachment_events(output: &std::path::Path) -> Vec<AttachmentEvent> {
    let path = output.with_file_name(subagent::ATTACHMENT_FILE_NAME);
    let mut reader = AttachmentReader::new(path);
    reader.read_new().expect("read attachment events")
}

#[tokio::test]
async fn sink_writes_prompt_and_starting_status() {
    let directory = TempDir::new().unwrap();
    let output = directory.path().join(subagent::RESULT_FILE_NAME);
    let sink = StatusSink::new(
        output.clone(),
        &identity(),
        "plan this",
        None,
        None,
        TEST_LABEL,
    )
    .unwrap();
    assert_eq!(sink.status().state, RunState::Starting);
    assert_eq!(sink.status().provider.as_deref(), Some("claude-code"));
    assert_eq!(sink.status().model.as_deref(), Some("opus"));
    assert_eq!(sink.status().reasoning, None);
    // Drop settles unfinished runs; inspect the journal prompt written at open.
    drop(sink);

    let events = read_attachment_events(&output);
    assert!(matches!(
        events.first(),
        Some(AttachmentEvent::Prompt(text)) if text == "plan this"
    ));
}

#[tokio::test]
async fn sink_applies_stream_effects_and_finalizes_success() {
    let directory = TempDir::new().unwrap();
    let output = directory.path().join(subagent::RESULT_FILE_NAME);
    let (tx, rx) = watch::channel(RunStatus::default());
    let mut sink = StatusSink::new(
        output.clone(),
        &identity(),
        "prompt",
        Some(tx),
        None,
        TEST_LABEL,
    )
    .unwrap();

    sink.mark_running();
    sink.apply_effect(StreamEffect::Attachment(AttachmentEvent::StepStarted));
    sink.apply_effect(StreamEffect::Status(StatusPatch {
        last_activity: Some("assistant".into()),
        state: Some(RunState::Running),
        ..StatusPatch::default()
    }));
    sink.apply_effect(StreamEffect::Attachment(
        AttachmentEvent::AssistantTextDelta("hello".into()),
    ));
    sink.finalize_success_from_stream(&success_terminal()).await;

    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Ok);
    assert_eq!(status.result.as_deref(), Some("done"));
    assert_eq!(status.claude_session_id.as_deref(), Some("sess"));
    assert_eq!(status.total_cost_usd, Some(0.12));
    assert_eq!(rx.borrow().state, RunState::Ok);

    let events = read_attachment_events(&output);
    assert!(events
        .iter()
        .any(|event| matches!(event, AttachmentEvent::Completed)));
    assert!(events.iter().any(
        |event| matches!(event, AttachmentEvent::AssistantTextDelta(text) if text == "hello")
    ));
}

#[tokio::test]
async fn sink_fail_and_stop_are_terminal() {
    let directory = TempDir::new().unwrap();
    let output = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut sink = StatusSink::new(
        output.clone(),
        &identity(),
        "prompt",
        None,
        None,
        TEST_LABEL,
    )
    .unwrap();
    sink.fail("boom").await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    assert_eq!(status.error.as_deref(), Some("boom"));
    assert!(read_attachment_events(&output)
        .iter()
        .any(|event| matches!(event, AttachmentEvent::Failed(text) if text == "boom")));

    let directory = TempDir::new().unwrap();
    let output = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut sink = StatusSink::new(
        output.clone(),
        &identity(),
        "prompt",
        None,
        None,
        TEST_LABEL,
    )
    .unwrap();
    sink.stop("cancelled", None).await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Stopped);
    assert_eq!(status.input_tokens, None);
    assert_eq!(status.output_tokens, None);
    assert!(read_attachment_events(&output)
        .iter()
        .any(|event| matches!(event, AttachmentEvent::Cancelled)));
}

#[tokio::test]
async fn second_terminal_finish_is_ignored() {
    let directory = TempDir::new().unwrap();
    let output = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut sink = StatusSink::new(
        output.clone(),
        &identity(),
        "prompt",
        None,
        None,
        TEST_LABEL,
    )
    .unwrap();
    sink.finalize_success_from_stream(&success_terminal()).await;
    sink.fail("later").await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Ok);
    assert_eq!(status.error, None);
}
