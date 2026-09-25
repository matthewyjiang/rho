use std::num::NonZeroU64;

use pretty_assertions::assert_eq;

use crate::{
    model::{ContentBlock, Message, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    CompactionOutput, CompactionPolicy, CompactionTrigger, Error, ProviderError, ProviderErrorKind,
    ProviderStreamResetReason, Retryability, Rho, RunEvent, ScriptedCompactor, SessionOptions,
    UserInput,
};

#[derive(Debug, PartialEq)]
enum Observed {
    Reset(ProviderStreamResetReason),
    CompactionStarted(CompactionTrigger),
    CompactionCompleted(CompactionTrigger),
}

fn overflow() -> ScriptedTurn {
    ScriptedTurn::failed(ProviderError::new(
        ProviderErrorKind::ContextOverflow,
        "request exceeds the model context window",
        Retryability::Permanent,
    ))
}

fn done() -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        "done".into(),
    )]))
}

// Covers: a provider context-overflow rejection compacts history once and
// retries with the replacement; a second overflow or a host without an
// automatic policy surfaces the original error instead of looping.
// Owner: SDK orchestration overflow recovery.
#[tokio::test]
async fn context_overflow_compacts_once_and_retries() {
    let recovery_events = vec![
        Observed::Reset(ProviderStreamResetReason::ContextOverflow),
        Observed::CompactionStarted(CompactionTrigger::ContextOverflow),
        Observed::CompactionCompleted(CompactionTrigger::ContextOverflow),
    ];
    let cases = [
        ("recovers", vec![overflow(), done()], true, 2, true),
        ("single retry", vec![overflow(), overflow()], true, 2, false),
        ("no policy", vec![overflow()], false, 1, false),
    ];

    for (case, turns, with_policy, expected_requests, succeeds) in cases {
        let identity = ModelIdentity::new("scripted", "test", "model");
        let provider = ScriptedProvider::new(identity, turns);
        let replacement = vec![
            Message::System("summary".into()),
            Message::user_text("current"),
        ];
        let mut builder =
            Rho::builder()
                .provider(provider.clone())
                .compactor(ScriptedCompactor::new([CompactionOutput::new(
                    replacement.clone(),
                )
                .unwrap()]));
        if with_policy {
            // Never due on its own, so only overflow recovery compacts.
            builder = builder.compaction_policy(CompactionPolicy::at_context_tokens(
                NonZeroU64::new(u64::MAX).unwrap(),
            ));
        }
        let session = builder
            .build()
            .unwrap()
            .session(SessionOptions::new().history(vec![
                Message::user_text("old ".repeat(2_000)),
                Message::assistant_text("old answer ".repeat(2_000)),
            ]))
            .await
            .unwrap();

        let mut run = session.start(UserInput::text("current")).await.unwrap();
        let mut observed = Vec::new();
        while let Some(event) = run.next_event().await {
            match event {
                RunEvent::ProviderStreamReset { reason, .. } => {
                    observed.push(Observed::Reset(reason));
                }
                RunEvent::CompactionStarted { trigger, .. } => {
                    observed.push(Observed::CompactionStarted(trigger));
                }
                RunEvent::CompactionCompleted { trigger, .. } => {
                    observed.push(Observed::CompactionCompleted(trigger));
                }
                _ => {}
            }
        }
        let outcome = run.outcome().await;
        let requests = provider.recorded_requests();

        assert_eq!(requests.len(), expected_requests, "{case}");
        if with_policy {
            assert_eq!(observed, recovery_events, "{case}");
            assert_eq!(requests[1].messages, replacement, "{case}");
        } else {
            assert_eq!(observed, Vec::new(), "{case}");
        }
        match outcome {
            Ok(outcome) => {
                assert!(succeeds, "{case}");
                assert_eq!(outcome.text(), "done", "{case}");
            }
            Err(Error::Provider(error)) => {
                assert!(!succeeds, "{case}");
                assert_eq!(error.kind(), ProviderErrorKind::ContextOverflow, "{case}");
            }
            Err(error) => panic!("{case}: unexpected error {error:?}"),
        }
    }
}
