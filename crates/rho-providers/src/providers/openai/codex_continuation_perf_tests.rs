//! Baseline-copy instructions in docs/performance-audit.md apply only to the
//! recorded audit revisions.

use std::{hint::black_box, time::Instant};

use super::{CodexContinuationCandidate, CodexContinuationResponse, CodexContinuationState};
use crate::model::{ContentBlock, ModelResponse};
use serde_json::json;

// Manual optimized benchmark, not a CI timing gate. Fixtures cover short and long
// transcripts with 2 KiB tool outputs. Construction is outside the timed region.
#[test]
#[ignore = "manual performance measurement; run with --release --ignored --nocapture"]
fn perf_audit_codex_candidate_bookkeeping() {
    let iterations = 20;
    for input_items in [64, 1024] {
        let body = json!({
            "model": "codex-benchmark",
            "instructions": "benchmark continuation bookkeeping",
            "input": (0..input_items).map(|index| json!({
                "type": "function_call_output",
                "call_id": format!("call_{index}"),
                "output": "x".repeat(2048),
            })).collect::<Vec<_>>(),
            "tools": [{"type":"function", "name":"read", "parameters":{"type":"object"}}],
            "store": false,
            "stream": true,
        });
        let response = CodexContinuationResponse::from_response(
            &ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
            Some("response_benchmark".into()),
            vec![
                json!({"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"done"}]}),
            ],
        );
        let mut state = CodexContinuationState::default();
        let mut samples_ns = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            for _ in 0..iterations {
                let candidate = CodexContinuationCandidate::from_responses_body(black_box(&body))
                    .expect("valid Responses body");
                // Includes replacement/drop of the preceding snapshot, just like real turns.
                state.record_success(candidate, black_box(response.clone()));
                black_box(&state);
            }
            samples_ns.push(started.elapsed().as_nanos());
        }
        println!(
            "{}",
            json!({
                "scenario": "codex_candidate_bookkeeping",
                "input_items": input_items,
                "output_bytes_per_item": 2048,
                "iterations_per_sample": iterations,
                "samples_ns": samples_ns,
            })
        );
    }
}
