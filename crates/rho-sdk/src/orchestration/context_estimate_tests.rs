use std::{num::NonZeroUsize, sync::Arc};

use pretty_assertions::assert_eq;
use tokio::sync::{mpsc, oneshot};

use super::execute_run;
use crate::{
    model::{
        context::estimate_context_tokens, ContentBlock, Message, ModelIdentity, ModelResponse,
    },
    provider::{ScriptedProvider, ScriptedTurn},
    run::RunCommand,
    session::{RunStart, SessionCore},
    CancellationToken, CompactionOutput, CompactionPolicy, CompactionState, Revision, Rho,
    RunEvent, RunId, ScriptedCompactor, SessionId, UserInput,
};

// Covers: StepStarted must describe the current request after compaction or
// staged steering, including a replacement with an unchanged message count.
// Owner: SDK orchestration. Existing compaction tests check history, not estimates.
#[tokio::test]
async fn step_context_estimate_tracks_compaction_and_staged_steering() {
    for (policy_threshold, steer) in [
        (None, false),
        (Some(3), false),
        (Some(3), true),
        (Some(2), false),
        (Some(2), true),
    ] {
        let provider = ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "context"),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("done".into()),
            ]))],
        );
        let mut builder = Rho::builder().provider(provider.clone());
        if let Some(threshold) = policy_threshold {
            builder = builder
                .compactor(ScriptedCompactor::new([CompactionOutput::new(vec![
                    Message::System("replacement".into()),
                    Message::user_text("current"),
                ])
                .unwrap()]))
                .compaction_policy(CompactionPolicy::after_messages(
                    NonZeroUsize::new(threshold).unwrap(),
                ));
        }
        let runtime = builder.build().unwrap();
        let core = SessionCore::new(
            SessionId::new(),
            vec![Message::System("old context ".repeat(100))],
            Revision::INITIAL,
            CompactionState::default(),
            /*metadata*/ Default::default(),
            /*prompt_cache_key*/ None,
            runtime.clone(),
        );
        // One queued command and rendezvous-like event backpressure make the
        // ordering deterministic, without sleeps or spawned timing races.
        let (commands, command_receiver) = mpsc::channel(1);
        if steer {
            let (accepted, _receipt) = oneshot::channel();
            commands
                .send(RunCommand::Steer {
                    input: UserInput::text("additional context ".repeat(100)),
                    accepted,
                })
                .await
                .unwrap();
        }
        drop(commands);
        let (events, mut event_receiver) = mpsc::channel(1);
        let worker = execute_run(
            Arc::clone(&core),
            runtime,
            RunId::new(),
            RunStart::user(UserInput::text("current")),
            CancellationToken::new(),
            events,
            command_receiver,
        );
        let receive = async {
            let mut estimates = Vec::new();
            while let Some(event) = event_receiver.recv().await {
                if let RunEvent::StepStarted {
                    estimated_context_tokens,
                    ..
                } = event
                {
                    estimates.push(estimated_context_tokens);
                }
            }
            estimates
        };
        let (outcome, estimates) = tokio::join!(worker, receive);
        outcome.unwrap();
        let requests = provider.recorded_requests();
        assert_eq!(
            estimates,
            requests
                .iter()
                .map(|request| estimate_context_tokens(&request.messages, &request.tools))
                .collect::<Vec<_>>()
        );
    }
}
