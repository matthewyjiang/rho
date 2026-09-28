use std::collections::HashSet;

use pretty_assertions::assert_eq;
use rho_sdk::model::{ContentBlock, Message, ToolCall, ToolResult};
use serde_json::json;

use super::*;

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
    }
}

fn read(id: &str, path: &str) -> ToolCall {
    call(id, "read_file", json!({ "path": path }))
}

fn turn(call: ToolCall, content: &str) -> [Message; 2] {
    let id = call.id.clone();
    [
        Message::Assistant(vec![ContentBlock::ToolCall(call)]),
        Message::ToolResult(ToolResult {
            id,
            ok: true,
            content: content.into(),
        }),
    ]
}

fn fingerprints(calls: &[ToolCall]) -> HashSet<ToolFingerprint> {
    calls.iter().filter_map(ToolFingerprint::of).collect()
}

// Covers: the proxy watches exactly the calls whose results the agent lost.
// A kept result is not a loss; an elision stub (same id, new content) and a
// summarized-away result are. Tools other than read_file and shells, and
// calls missing the identifying argument, are never watched.
// Owner: compaction metrics.
#[test]
fn removed_tool_calls_are_the_ones_whose_results_left_history() {
    let kept = read("kept", "src/kept.rs");
    let elided = read("elided", "src/elided.rs");
    let summarized = call("summarized", "bash", json!({ "command": "cargo test" }));
    let unwatched = call("grep", "grep", json!({ "pattern": "x" }));
    let no_path = call("no_path", "read_file", json!({}));
    let before = [
        turn(kept.clone(), "kept text"),
        turn(elided.clone(), "elided text"),
        turn(summarized.clone(), "test output"),
        turn(unwatched, "matches"),
        turn(no_path, "error"),
    ]
    .concat();
    let after = [
        turn(kept, "kept text"),
        turn(elided.clone(), "[elided tool result]"),
    ]
    .concat();

    assert_eq!(
        removed_tool_calls(&before, &after),
        fingerprints(&[elided, summarized])
    );
}

fn completed() -> CompactionRecord {
    CompactionRecord {
        identity: RecordIdentity::default(),
        outcome: CompactionRunOutcome::Completed,
        trigger: CompactionTriggerKind::Automatic,
        tier: Some(CompactionTier::Elision),
        request_path: None,
        model: None,
        elided_tool_results: 1,
        context_tokens: 1_000,
        prompt_tokens: None,
        output_tokens: None,
        cache_read_tokens: None,
        cost_usd_micros: None,
        latency_ms: 1,
        next_prompt_tokens: None,
        reread: None,
    }
}

fn reread(tool_calls: u32, repeated: u32) -> Option<RereadStats> {
    Some(RereadStats {
        window: REREAD_WINDOW_TOOL_CALLS,
        tool_calls,
        repeated,
        tracked: 1,
    })
}

// Covers: follow-up signals count only after the SDK commits, repeats are
// counted by fingerprint (whitespace-trimmed), the window caps the count, and
// the first prompt report wins. The record is handed back for saving when the
// prompt size arrives (without a partial re-read count, which would stick in
// the ledger) and once more when the window completes.
// Owner: compaction metrics.
#[test]
fn follow_up_collects_after_commit_and_finishes_once() {
    let removed = read("old", "src/lib.rs");
    let repeat = read("new", " src/lib.rs ");
    let other = read("other", "src/main.rs");
    let mut metrics = CompactionMetrics::default();
    assert_eq!(metrics.record(completed(), fingerprints(&[removed])), None);

    // Before the commit, nothing counts.
    assert_eq!(metrics.observe_tool_call(&repeat), None);
    assert_eq!(metrics.observe_prompt_tokens(Some(1)), None);
    assert_eq!(metrics.last().unwrap().reread, None);

    metrics.committed();
    assert_eq!(metrics.observe_tool_call(&repeat), None);
    let early = metrics.observe_prompt_tokens(Some(500)).unwrap();
    assert_eq!((early.next_prompt_tokens, early.reread), (Some(500), None));
    assert_eq!(metrics.observe_prompt_tokens(Some(900)), None);
    for _ in 1..REREAD_WINDOW_TOOL_CALLS - 1 {
        assert_eq!(metrics.observe_tool_call(&other), None);
    }
    let finished = metrics.observe_tool_call(&repeat).unwrap();

    assert_eq!(
        (finished.next_prompt_tokens, finished.reread),
        (Some(500), reread(REREAD_WINDOW_TOOL_CALLS, 2))
    );
    assert_eq!(metrics.observe_tool_call(&repeat), None);
    assert_eq!(metrics.last(), Some(&finished));
}

// Covers: a follow-up cut short (by a newer compaction or a session change) is
// handed back once so its partial numbers are saved; a failed compaction or
// one that never committed has no follow-up to hand back.
// Owner: compaction metrics.
#[test]
fn cut_short_follow_up_is_returned_for_saving() {
    let failed = CompactionRecord {
        outcome: CompactionRunOutcome::Failed,
        ..completed()
    };
    let following = || {
        let mut metrics = CompactionMetrics::default();
        metrics.record(completed(), HashSet::new());
        metrics.committed();
        metrics.observe_tool_call(&read("a", "a"));
        metrics
    };
    let partial = CompactionRecord {
        reread: Some(RereadStats {
            tracked: 0,
            ..reread(1, 0).unwrap()
        }),
        ..completed()
    };

    let mut superseded = following();
    assert_eq!(
        superseded.record(completed(), HashSet::new()),
        Some(partial.clone())
    );
    let mut switched = following();
    assert_eq!(switched.take_unfinished(), Some(partial));
    assert_eq!(switched.take_unfinished(), None);

    let mut never_committed = CompactionMetrics::default();
    never_committed.record(completed(), HashSet::new());
    assert_eq!(never_committed.take_unfinished(), None);

    let mut failed_metrics = CompactionMetrics::default();
    failed_metrics.record(failed, HashSet::new());
    failed_metrics.committed();
    assert_eq!(failed_metrics.last().unwrap().reread, None);
    assert_eq!(failed_metrics.take_unfinished(), None);
}
