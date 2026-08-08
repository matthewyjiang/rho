use pretty_assertions::assert_eq;
use tempfile::TempDir;

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
        model: "test-model".into(),
        runtime: crate::agent::AgentRuntime::Rho,
    }
}

// Covers: a stream far larger than the writer queue must keep `rho attach`
// recording alive and replay the exact ordered text, even when the sink
// coalesces bursts.
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
