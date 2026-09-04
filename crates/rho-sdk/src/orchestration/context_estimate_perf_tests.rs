//! Baseline-copy instructions in docs/performance-audit.md apply only to the
//! recorded audit revisions.

use std::{hint::black_box, num::NonZeroU64, time::Duration, time::Instant};

use crate::{
    model::{
        AssistantMessage, ContentBlock, Message, ModelIdentity, ModelRequest, ModelResponse,
        ProviderContextBlock,
    },
    provider::{ModelProvider, ProviderFuture},
    CompactionPolicy, Rho, ScriptedCompactor, SessionOptions,
};

struct ImmediateProvider;

impl ModelProvider for ImmediateProvider {
    fn identity(&self) -> ModelIdentity {
        ModelIdentity::new("benchmark", "context", "immediate")
    }

    fn send_turn<'a>(&'a self, _request: ModelRequest<'a>) -> ProviderFuture<'a> {
        Box::pin(async {
            Ok(ModelResponse::Assistant(vec![ContentBlock::Text(
                "ok".into(),
            )]))
        })
    }
}

// Exercises execute_turn_loop, not a benchmark-side imitation of estimate reuse.
// Session construction is excluded; run startup, history snapshots, events and
// terminal commit are included in both policy modes. Opaque replay JSON makes
// history estimation material, as with stored provider reasoning context.
// This file and its module declaration work unchanged on baseline 06d1ff28.
#[tokio::test]
#[ignore = "manual performance measurement; run with --release --ignored --nocapture"]
async fn perf_audit_context_estimation_reuse() {
    let messages = 256;
    let replay_bytes = 8192;
    let iterations = 10;
    let identity = ImmediateProvider.identity();
    let history = (0..messages)
        .map(|_| {
            Message::assistant(AssistantMessage {
                content: vec![ContentBlock::Text("answer".into())],
                provenance: Some(identity.clone()),
                reasoning_summary: None,
                provider_context: vec![ProviderContextBlock {
                    identity: identity.clone(),
                    kind: "benchmark_replay".into(),
                    position: None,
                    data: serde_json::json!({"encrypted_content": "x".repeat(replay_bytes)}),
                }],
            })
        })
        .collect::<Vec<_>>();
    for compaction_enabled in [false, true] {
        let mut builder = Rho::builder().provider(ImmediateProvider);
        if compaction_enabled {
            // Deliberately unreachable trigger: this measures the no-compaction
            // policy check, not a model-backed compaction request.
            builder = builder
                .compactor(ScriptedCompactor::new([]))
                .compaction_policy(CompactionPolicy::at_context_tokens(NonZeroU64::MAX));
        }
        let runtime = builder.build().unwrap();
        let mut samples_ns = Vec::new();
        for _ in 0..5 {
            let mut elapsed = Duration::ZERO;
            for _ in 0..iterations {
                let session = runtime
                    .session(SessionOptions::new().history(history.clone()))
                    .await
                    .unwrap();
                let started = Instant::now();
                black_box(session.complete("next").await.unwrap());
                elapsed += started.elapsed();
            }
            samples_ns.push(elapsed.as_nanos());
        }
        println!(
            "{}",
            serde_json::json!({
                "scenario": "context_estimation_reuse",
                "compaction_enabled": compaction_enabled,
                "messages": messages,
                "replay_bytes_per_message": replay_bytes,
                "iterations_per_sample": iterations,
                "samples_ns": samples_ns,
            })
        );
    }
}
