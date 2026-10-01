use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::watch;

use super::*;
use crate::run_artifacts::AttachmentReader;

/// Journal shape a replaying reader observes, with adjacent text deltas merged.
///
/// Coalescing may reduce how many delta events reach disk, so the number of
/// events is not part of the contract. The concatenated text and its position
/// relative to every other event are.
#[derive(Debug, PartialEq)]
enum Replayed {
    Prompt(String),
    Text(String),
    Notice(String),
    Completed,
    Other(String),
}

fn replay(path: &Path) -> Vec<Replayed> {
    let mut reader = AttachmentReader::new(path.to_path_buf());
    let mut replayed: Vec<Replayed> = Vec::new();
    for event in reader.read_new().unwrap() {
        let next = match event {
            AttachmentEvent::Prompt(text) => Replayed::Prompt(text),
            AttachmentEvent::AssistantTextDelta(text) => {
                if let Some(Replayed::Text(previous)) = replayed.last_mut() {
                    previous.push_str(&text);
                    continue;
                }
                Replayed::Text(text)
            }
            AttachmentEvent::Notice(text) => Replayed::Notice(text),
            AttachmentEvent::Completed => Replayed::Completed,
            other => Replayed::Other(format!("{other:?}")),
        };
        replayed.push(next);
    }
    replayed
}

fn test_identity() -> RunArtifactIdentity {
    RunArtifactIdentity {
        agent_id: "alpha".into(),
        agent_fingerprint: "fingerprint".into(),
        provider: "test".into(),
        model: Some("test-model".into()),
        runtime: crate::agent::AgentRuntime::Rho,
        reasoning: None,
    }
}

// Covers: a stream far larger than the writer queue must keep `rho attach`
// recording alive and replay the exact ordered text, even when the sink
// coalesces bursts. Windows journal flush is slower; the sink budget must
// still accept the burst instead of disabling recording.
// Owner: run-artifact sink (writer queue backpressure and journal replay)
#[test]
fn burst_of_deltas_keeps_recording_and_replays_losslessly() {
    const DELTAS: usize = 5_000;
    const NOTICE_AFTER: usize = DELTAS / 2;

    let directory = TempDir::new().unwrap();
    let path = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut sink = RunArtifactSink::open(path.clone(), &test_identity(), "prompt", None).unwrap();

    let mut before_notice = String::new();
    let mut after_notice = String::new();
    for index in 0..DELTAS {
        let text = format!("chunk-{index} ");
        if index <= NOTICE_AFTER {
            before_notice.push_str(&text);
        } else {
            after_notice.push_str(&text);
        }
        sink.write_attachment(AttachmentEvent::AssistantTextDelta(text));
        if index == NOTICE_AFTER {
            sink.write_attachment(AttachmentEvent::Notice("halfway".into()));
        }
    }
    sink.finish_ok(Some("done".into()));

    assert_eq!(sink.status.attachment_error, None);
    assert_eq!(sink.status.state, RunState::Ok);
    assert_eq!(
        replay(&path.with_file_name(subagent::ATTACHMENT_FILE_NAME)),
        vec![
            Replayed::Prompt("prompt".into()),
            Replayed::Text(before_notice),
            Replayed::Notice("halfway".into()),
            Replayed::Text(after_notice),
            Replayed::Completed,
        ]
    );
}

// Covers: a detached writer must eventually close its journal even after queue
// overflow, retaining the truncation diagnostic. Terminal status alone is not
// a writer-completion signal.
// Owner: run-artifact sink (finish ordering and queue backpressure)
#[tokio::test]
async fn detached_finish_keeps_terminal_attachment_after_queue_overflow() {
    use crate::run_artifacts::test_support::wait_for_writer;

    fn queue_pause(sink: &RunArtifactSink) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (resume_tx, resume_rx) = mpsc::channel();
        assert!(sink
            .tx
            .as_ref()
            .unwrap()
            .try_send(WriterCommand::Pause {
                entered: entered_tx,
                resume: resume_rx,
            })
            .is_ok());
        (entered_rx, resume_tx)
    }

    // Use the process-session failure bound only around explicit gate signals.
    const GATE_BUDGET: Duration = Duration::from_secs(30);
    for overflow in [false, true] {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join(subagent::RESULT_FILE_NAME);
        let (status_tx, status_rx) = watch::channel(RunStatus::default());
        let mut sink =
            RunArtifactSink::open(path.clone(), &test_identity(), "prompt", Some(status_tx))
                .unwrap();
        let (entered, mut resume) = queue_pause(&sink);
        entered.recv_timeout(GATE_BUDGET).unwrap();

        if overflow {
            // Fill every slot while the writer is held, then force a real
            // enqueue-budget failure. A second gate lets the queue drain before
            // Finish without letting its terminal event reach disk yet.
            for _ in 0..QUEUE_CAPACITY - 1 {
                assert!(sink.enqueue(WriterCommand::Status(sink.status.clone())));
            }
            let (entered, next_resume) = queue_pause(&sink);
            sink.write_attachment(AttachmentEvent::Notice("overflow".into()));
            assert!(sink.status.attachment_error.is_some());
            assert!(!sink.attachment_enabled);
            resume.send(()).unwrap();
            resume = next_resume;
            entered.recv_timeout(GATE_BUDGET).unwrap();
        }

        sink.status.state = RunState::Ok;
        sink.status.mark_finished_now();
        // The gate, not elapsed time, holds the writer beyond this budget.
        sink.finish_with_join_budget(Some(AttachmentEvent::Completed), Duration::ZERO);
        let early_status = subagent::read_status(&path).unwrap();
        assert_eq!(early_status.state, RunState::Ok);
        assert_eq!(early_status.attachment_error.is_some(), overflow);
        let journal = path.with_file_name(subagent::ATTACHMENT_FILE_NAME);
        assert_eq!(replay(&journal), vec![Replayed::Prompt("prompt".into())]);
        drop(sink);
        assert!(
            status_rx.has_changed().is_ok(),
            "detached writer owns sender"
        );

        resume.send(()).unwrap();
        let final_status = wait_for_writer(status_rx).await;
        assert_eq!(final_status.attachment_error, early_status.attachment_error);
        assert_eq!(subagent::read_status(&path).unwrap(), final_status);
        assert_eq!(
            replay(&journal),
            vec![Replayed::Prompt("prompt".into()), Replayed::Completed],
            "overflow: {overflow}; attachment_error: {:?}",
            final_status.attachment_error
        );
    }
}

// Covers: reasoning and assistant text are separate streams, so coalescing must
// never fold one into the other or reorder them.
// Owner: run-artifact sink (journal replay)
#[test]
fn coalescing_keeps_reasoning_and_text_streams_separate() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join(subagent::RESULT_FILE_NAME);
    let mut sink = RunArtifactSink::open(path.clone(), &test_identity(), "prompt", None).unwrap();

    for _ in 0..512 {
        sink.write_attachment(AttachmentEvent::ReasoningDelta("think ".into()));
        sink.write_attachment(AttachmentEvent::AssistantTextDelta("say ".into()));
    }
    sink.finish_ok(None);

    assert_eq!(sink.status.attachment_error, None);
    let events = {
        let mut reader = AttachmentReader::new(path.with_file_name(subagent::ATTACHMENT_FILE_NAME));
        reader.read_new().unwrap()
    };
    let mut reasoning = String::new();
    let mut text = String::new();
    let mut interleavings = 0_usize;
    let mut last_was_reasoning = false;
    for event in &events {
        match event {
            AttachmentEvent::ReasoningDelta(chunk) => {
                reasoning.push_str(chunk);
                if !last_was_reasoning {
                    interleavings += 1;
                }
                last_was_reasoning = true;
            }
            AttachmentEvent::AssistantTextDelta(chunk) => {
                text.push_str(chunk);
                last_was_reasoning = false;
            }
            _ => {}
        }
    }
    assert_eq!(reasoning, "think ".repeat(512));
    assert_eq!(text, "say ".repeat(512));
    // Every reasoning run is followed by text, so the streams stay ordered pairs
    // no matter how many deltas merged inside each run.
    assert_eq!(interleavings, 512);
}

// Covers: a title that arrives while the sink is constructed must survive the
// first watch publish, not only the on-disk result file.
// Owner: run-artifact sink
#[test]
fn continue_from_keeps_a_title_written_during_construction() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join(subagent::RESULT_FILE_NAME);
    let started = RunStatus {
        state: RunState::Starting,
        agent_id: Some("worker".into()),
        last_activity: Some("starting".into()),
        ..RunStatus::default()
    };
    subagent::initialize_status(&path, &started).unwrap();
    let (tx, rx) = watch::channel(started.clone());
    let live_title = Arc::new(Mutex::new(Some("Review the auth path".into())));

    let mut sink = RunArtifactSink::continue_from(
        path,
        started,
        "prompt",
        Some(tx),
        Some(Arc::clone(&live_title)),
    )
    .unwrap();
    assert_eq!(rx.borrow().title.as_deref(), Some("Review the auth path"));
    assert_eq!(rx.borrow().last_activity.as_deref(), Some("starting"));

    sink.mark_running("tool: read");

    assert_eq!(sink.status.title.as_deref(), Some("Review the auth path"));
    assert_eq!(rx.borrow().title.as_deref(), Some("Review the auth path"));
    assert_eq!(rx.borrow().last_activity.as_deref(), Some("tool: read"));
}
