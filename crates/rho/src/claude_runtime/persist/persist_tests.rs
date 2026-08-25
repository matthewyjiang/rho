use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::watch;

use super::{ClaudeRunIdentity, StatusSink};
use crate::claude_runtime::stream::{
    StatusPatch, StreamEffect, TerminalClassification, TerminalResult,
};
use crate::{
    run_artifacts::{AttachmentEvent, AttachmentReader},
    subagent::{self, RunState, RunStatus},
};

fn identity() -> ClaudeRunIdentity {
    ClaudeRunIdentity {
        agent_id: "planner".into(),
        agent_fingerprint: "fp".into(),
        model: Some("opus".into()),
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
    let sink = StatusSink::new(output.clone(), &identity(), "plan this", None, None).unwrap();
    assert_eq!(sink.status().state, RunState::Starting);
    assert_eq!(sink.status().provider.as_deref(), Some("claude-code"));
    assert_eq!(sink.status().model.as_deref(), Some("opus"));
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
    let mut sink = StatusSink::new(output.clone(), &identity(), "prompt", Some(tx), None).unwrap();

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
    let mut sink = StatusSink::new(output.clone(), &identity(), "prompt", None, None).unwrap();
    sink.fail("boom").await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Error);
    assert_eq!(status.error.as_deref(), Some("boom"));
    assert!(read_attachment_events(&output)
        .iter()
        .any(|event| matches!(event, AttachmentEvent::Failed(text) if text == "boom")));

    let directory = TempDir::new().unwrap();
    let output = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut sink = StatusSink::new(output.clone(), &identity(), "prompt", None, None).unwrap();
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
async fn sink_collects_rate_limits_until_settle() {
    let directory = TempDir::new().unwrap();
    let rate_limit_path = directory.path().join("rate-limits.json");
    let output = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut sink = StatusSink::new(
        output.clone(),
        &identity(),
        "prompt",
        None,
        Some(rate_limit_path.clone()),
    )
    .unwrap();
    sink.apply_effect(StreamEffect::RateLimit(
        crate::claude_runtime::stream::RateLimitInfo {
            status: Some("allowed".into()),
            rate_limit_type: Some("five_hour".into()),
            resets_at: Some(1_700_000_000),
            utilization: Some(0.25),
            overage_status: None,
            overage_resets_at: None,
            is_using_overage: None,
        },
    ));
    // Cache is empty until settle.
    assert!(crate::claude_runtime::rate_limit::load_at(&rate_limit_path).is_none());
    sink.finalize_success_from_stream(&success_terminal()).await;
    let state = crate::claude_runtime::rate_limit::load_at(&rate_limit_path).expect("cached");
    assert_eq!(state.windows.len(), 1);
    assert_eq!(state.windows[0].info.window_key(), "five_hour");
}

#[tokio::test]
async fn second_terminal_finish_is_ignored() {
    let directory = TempDir::new().unwrap();
    let output = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut sink = StatusSink::new(output.clone(), &identity(), "prompt", None, None).unwrap();
    sink.finalize_success_from_stream(&success_terminal()).await;
    sink.fail("later").await;
    let status = subagent::read_status(&output).expect("status");
    assert_eq!(status.state, RunState::Ok);
    assert_eq!(status.error, None);
}
