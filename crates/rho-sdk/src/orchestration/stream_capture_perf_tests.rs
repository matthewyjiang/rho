//! Baseline-copy instructions in docs/performance-audit.md apply only to the
//! recorded audit revisions.

use std::{hint::black_box, time::Instant};

use super::{capture_provider_event, StreamCapture};
use crate::model::{ModelEvent, ModelIdentity, ModelUsage};

// Measures the real capture/forwarding path including event String ownership.
// 64-byte deltas model a fragmented 1 MiB reasoning stream. No timing assertions.
#[test]
#[ignore = "manual performance measurement; run with --release --ignored --nocapture"]
fn perf_audit_reasoning_capture() {
    let identity = ModelIdentity::new("benchmark", "capture", "reasoning");
    let usage = ModelUsage::default();
    let delta = "x".repeat(64);
    // A larger one-pass workload also supports external peak-RSS measurement.
    let chunks = workload("RHO_BENCH_REASONING_CHUNKS", 16_384);
    let iterations = workload("RHO_BENCH_ITERATIONS", 20);
    let mut samples_ns = Vec::new();
    for _ in 0..workload("RHO_BENCH_SAMPLES", 5) {
        let started = Instant::now();
        for _ in 0..iterations {
            let mut capture = StreamCapture::default();
            for _ in 0..chunks {
                black_box(capture_provider_event(
                    ModelEvent::ReasoningDelta(black_box(&delta).clone()),
                    &identity,
                    &usage,
                    &mut capture,
                ));
            }
            black_box(&capture);
            black_box(capture.into_aborted_assistant());
        }
        samples_ns.push(started.elapsed().as_nanos());
    }
    println!(
        "{}",
        serde_json::json!({
            "scenario": "reasoning_capture",
            "delta_bytes": delta.len(),
            "chunks": chunks,
            "iterations_per_sample": iterations,
            "samples_ns": samples_ns,
        })
    );
}

fn workload(name: &str, default: usize) -> usize {
    let value = std::env::var(name)
        .map(|value| {
            value
                .parse()
                .expect("benchmark workload must be an integer")
        })
        .unwrap_or(default);
    assert!(value > 0, "{name} must be positive");
    value
}
