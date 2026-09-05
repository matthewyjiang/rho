use std::num::NonZeroUsize;

use pretty_assertions::assert_eq;

use crate::{
    boundary_input_channel,
    model::{ContentBlock, Message, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    Error, InputBoundary, Rho, RunEvent, SessionOptions, UserInput,
};

// Covers: input pending at the final checkpoint must cause another provider step,
// with identity distinct from human steering, even under event backpressure.
// Consecutive injections must not suppress compaction or lose verbatim input.
// Owner: SDK orchestration. PTY covers CLI collection after a real process exits.
#[tokio::test]
async fn completion_checkpoint_incorporates_input_before_committing() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        ["candidate", "candidate", "incorporated"].map(|text| {
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                text.into(),
            )]))
        }),
    );
    let summary = Message::user_text("summary");
    // The prefix alone is below the threshold; fresh input must still count
    // toward the live request size when deciding whether to compact.
    let threshold = crate::model::context::estimate_context_tokens(
        &[
            Message::user_text("work"),
            Message::assistant_text("candidate"),
        ],
        &[],
    ) + 1;
    let runtime = Rho::builder()
        .provider(provider.clone())
        .compactor(crate::ScriptedCompactor::new((0..2).map(|_| {
            crate::CompactionOutput::new(vec![summary.clone()]).unwrap()
        })))
        .compaction_policy(crate::CompactionPolicy::at_context_tokens(
            std::num::NonZeroU64::new(threshold).unwrap(),
        ))
        .event_capacity(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let (source, mut requests) = boundary_input_channel();
    session.set_boundary_inputs(Some(source)).unwrap();
    let mut run = session.start(UserInput::text("work")).await.unwrap();
    let inputs = ["internal failure", "another internal failure"];
    let mut pending = inputs.into_iter().map(UserInput::text);
    let mut boundaries = Vec::new();
    let mut applied = Vec::new();
    let mut completed = Vec::new();
    let mut compacted = Vec::new();
    loop {
        tokio::select! {
            Some(request) = requests.recv() => {
                assert_eq!((request.session_id(), request.run_id()), (session.id(), run.id()));
                let boundary = request.boundary();
                boundaries.push(boundary);
                let input = if boundary == InputBoundary::BeforeCompletion { pending.next() } else { None };
                assert!(request.respond(input).await);
            }
            event = run.next_event() => match event {
                Some(RunEvent::BoundaryInputApplied { session_id, run_id, input }) => applied.push((session_id, run_id, input)),
                Some(RunEvent::Completed { outcome }) => completed.push(outcome.text().to_owned()),
                Some(RunEvent::CompactionCompleted { outcome, .. }) => {
                    compacted.push(outcome.committed_snapshot().unwrap().history().to_vec());
                }
                Some(_) => {}
                None => break,
            }
        }
    }
    assert_eq!(
        boundaries,
        vec![
            InputBoundary::BeforeProvider,
            InputBoundary::BeforeCompletion,
            InputBoundary::BeforeProvider,
            InputBoundary::BeforeCompletion,
            InputBoundary::BeforeProvider,
            InputBoundary::BeforeCompletion
        ]
    );
    assert_eq!(
        applied,
        inputs
            .map(|text| (
                session.id().clone(),
                run.id().clone(),
                UserInput::text(text)
            ))
            .to_vec()
    );
    assert_eq!(completed, vec!["incorporated"]);
    assert_eq!(run.outcome().await.unwrap().text(), "incorporated");
    let expected = inputs.map(|text| vec![summary.clone(), Message::user_text(text)]);
    assert_eq!(compacted, expected);
    let requests_seen = provider.recorded_requests();
    assert_eq!(requests_seen.len(), 3);
    assert_eq!(requests_seen[1].messages, expected[0]);
    assert_eq!(requests_seen[2].messages, expected[1]);
    // Empty final acknowledgement closes the checkpoint stream for this run.
    assert!(requests.try_recv().is_err());
}

// Covers: the final step must leave completion notices queued with the host.
// Owner: SDK orchestration step budget.
#[tokio::test]
async fn last_step_does_not_accept_completion_input() {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("scripted", "test", "model"),
        [ScriptedTurn::completed(ModelResponse::Assistant(vec![
            ContentBlock::Text("done".into()),
        ]))],
    );
    let runtime = Rho::builder()
        .provider(provider.clone())
        .max_steps(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::default()).await.unwrap();
    let (source, mut requests) = boundary_input_channel();
    session.set_boundary_inputs(Some(source)).unwrap();
    let mut run = session.start(UserInput::text("work")).await.unwrap();
    let mut boundaries = Vec::new();
    loop {
        tokio::select! {
            Some(request) = requests.recv() => {
                boundaries.push(request.boundary());
                let input = (request.boundary() == InputBoundary::BeforeCompletion)
                    .then(|| UserInput::text("must stay queued"));
                request.respond(input).await;
            }
            event = run.next_event() => if event.is_none() { break; },
        }
    }
    assert_eq!(boundaries, vec![InputBoundary::BeforeProvider]);
    let outcome = run.outcome().await.unwrap();
    assert_eq!(outcome.text(), "done");
    assert_eq!(outcome.stop_reason(), crate::StopReason::EndTurn);
    assert_eq!(provider.recorded_requests().len(), 1);
}

// Covers: cancellation/host disappearance cannot leave the SDK parked forever,
// and a cancelled reservation must not be reported accepted.
// Owner: SDK checkpoint protocol.
#[tokio::test]
async fn abandoned_checkpoint_fails_without_accepting_input() {
    for cancel in [true, false] {
        let runtime = Rho::builder()
            .provider(ScriptedProvider::new(
                ModelIdentity::new("scripted", "test", "model"),
                [],
            ))
            .build()
            .unwrap();
        let session = runtime.session(SessionOptions::default()).await.unwrap();
        let (source, mut requests) = boundary_input_channel();
        session.set_boundary_inputs(Some(source)).unwrap();
        let mut run = session.start(UserInput::text("work")).await.unwrap();
        let request = requests.recv().await.unwrap();
        if cancel {
            run.cancel();
            while run.next_event().await.is_some() {}
            assert!(!request.respond(Some(UserInput::text("restore me"))).await);
            assert!(matches!(run.outcome().await, Err(Error::Cancelled)));
        } else {
            drop(request);
            drop(requests);
            while run.next_event().await.is_some() {}
            assert!(matches!(
                run.outcome().await,
                Err(Error::Interrupted { .. })
            ));
        }
        assert!(!session.is_running());
    }
}

// Covers: the event consumer disappearing after acceptance must not discard an
// unseen failure that the host has already removed from its notification queue.
// Owner: SDK checkpoint durability, exercising the uncommitted Interrupted path.
#[tokio::test]
async fn accepted_input_survives_event_consumer_loss() {
    use crate::{
        session::{RunStart, SessionCore},
        CancellationToken, Revision, RunId,
    };
    use std::sync::Arc;

    let mut runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "model"),
            [],
        ))
        .build()
        .unwrap();
    let (source, mut requests) = boundary_input_channel();
    runtime.boundary_inputs = Some(source);
    let core = SessionCore::new(
        crate::SessionId::new(),
        Vec::new(),
        Revision::INITIAL,
        Default::default(),
        Default::default(),
        None,
        runtime.clone(),
    );
    // Started fills the event buffer, forcing BoundaryInputApplied to wait
    // after accepting the reply. No scheduling delay is needed to hit the gap.
    let (events, receiver) = tokio::sync::mpsc::channel(1);
    let (_commands, command_receiver) = tokio::sync::mpsc::channel(1);
    let worker = tokio::spawn(crate::orchestration::execute_run(
        Arc::clone(&core),
        runtime,
        RunId::new(),
        RunStart::user(UserInput::text("work")),
        CancellationToken::new(),
        events,
        command_receiver,
    ));
    let request = requests.recv().await.unwrap();
    assert!(
        request
            .respond(Some(UserInput::text("unseen failure")))
            .await
    );
    let expected = vec![
        Message::user_text("work"),
        Message::user_text("unseen failure"),
    ];
    assert_eq!(core.snapshot().0, expected);
    assert_eq!(core.snapshot().1, Revision::INITIAL.checked_next().unwrap());
    drop(receiver);
    assert!(matches!(
        worker.await.unwrap(),
        Err(Error::Interrupted { .. })
    ));
    assert_eq!(core.persistence_snapshot().history(), expected);
}
