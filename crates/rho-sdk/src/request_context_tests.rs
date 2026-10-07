use std::sync::{Arc, Mutex};

use pretty_assertions::assert_eq;

use crate::{
    model::{
        context::estimate_context_tokens, ContentBlock, Message, ModelEvent, ModelIdentity,
        ModelResponse, ModelUsage,
    },
    provider::{ModelProvider, ScriptedProvider, ScriptedTurn},
    ContextEstimate, RequestContext, Rho, SessionId, SessionOptions,
};

#[derive(Clone)]
struct LiveContext(Arc<Mutex<Vec<Message>>>);

impl RequestContext for LiveContext {
    fn messages(&self, _session_id: &SessionId) -> Vec<Message> {
        self.0.lock().unwrap().clone()
    }
}

fn reply() -> ScriptedTurn {
    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
        "done".into(),
    )]))
}

// Covers: request-only context must not pollute history, move the prompt prefix,
// be duplicated on resume, or discard calibration on unchanged-context appends.
// Replacing the source must invalidate the measured anchor before the next request.
// Owner: SDK request projection and context accounting.
#[tokio::test]
async fn live_context_tracks_requests_without_polluting_history_or_calibration() {
    let source = LiveContext(Arc::new(Mutex::new(vec![Message::model_context(
        "first context",
    )])));
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            ScriptedTurn::streaming(
                vec![ModelEvent::Usage(ModelUsage {
                    input_tokens: Some(1_000),
                    ..ModelUsage::default()
                })],
                ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
            ),
            reply(),
            reply(),
            reply(),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider.clone())
        .request_context(source.clone())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::new()).await.unwrap();
    session.complete("first human request").await.unwrap();
    session.complete("second human request").await.unwrap();
    let estimate = session.context_estimate();
    assert_eq!(estimate.provider_reported_tokens(), Some(1_000));
    let history = session.history();
    let projected = runtime.request_messages(session.id(), &history);
    assert_eq!(
        estimate.estimated_tokens(),
        estimate_context_tokens(&projected.messages, &[])
    );

    *source.0.lock().unwrap() = vec![Message::model_context("replacement context")];
    assert_eq!(session.context_estimate().provider_reported_tokens(), None);
    session.complete("third human request").await.unwrap();
    let snapshot = session.snapshot();
    assert_eq!(
        snapshot
            .history()
            .iter()
            .filter(|message| message.as_model_context().is_some())
            .count(),
        0
    );
    let restored = runtime
        .session(SessionOptions::from_snapshot(snapshot))
        .await
        .unwrap();
    restored.complete("resumed human request").await.unwrap();
    let requests = provider.recorded_requests();
    assert_eq!(requests.len(), 4);
    let expected = [
        "first context",
        "first context",
        "replacement context",
        "replacement context",
    ];
    for (request, expected) in requests.iter().zip(expected) {
        assert_eq!(
            request.messages.last().and_then(Message::as_model_context),
            Some(expected)
        );
        assert_eq!(
            request
                .messages
                .iter()
                .filter(|message| message.as_model_context().is_some())
                .count(),
            1
        );
    }
    assert_eq!(
        &requests[1].messages[..requests[0].messages.len() - 1],
        &requests[0].messages[..requests[0].messages.len() - 1]
    );
    let history = restored.history();
    let projected = runtime.request_messages(restored.id(), &history);
    assert_eq!(
        restored.context_estimate(),
        ContextEstimate::from_estimated_tokens(estimate_context_tokens(&projected.messages, &[]))
    );
}

// Covers: source replacement/removal must permanently discard calibration, even
// after reverting to the old source, without double-counting raw-history appends.
// Owner: SDK session accounting. The request/history test above covers stable sources.
#[tokio::test]
async fn source_changes_invalidate_calibration_across_empty_transitions() {
    let first = vec![Message::model_context("first context")];
    let other = vec![Message::model_context("other context")];
    for (initial, replacement) in [
        (Vec::new(), first.clone()),
        (first.clone(), Vec::new()),
        (first, other),
    ] {
        let source = LiveContext(Arc::new(Mutex::new(initial.clone())));
        let runtime = Rho::builder()
            .provider(ScriptedProvider::new(
                ModelIdentity::new("test", "test", "test"),
                [ScriptedTurn::streaming(
                    vec![ModelEvent::Usage(ModelUsage {
                        input_tokens: Some(1_000),
                        ..ModelUsage::default()
                    })],
                    ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
                )],
            ))
            .request_context(source.clone())
            .build()
            .unwrap();
        let session = runtime.session(SessionOptions::new()).await.unwrap();
        session.complete("request").await.unwrap();
        assert_eq!(
            session.context_estimate().provider_reported_tokens(),
            Some(1_000)
        );

        *source.0.lock().unwrap() = replacement;
        let changed = session.context_estimate();
        assert_eq!(changed.provider_reported_tokens(), None);
        assert_eq!(changed, session.estimate_context(&session.history()));
        assert_eq!(session.context_estimate(), changed);
        *source.0.lock().unwrap() = initial;
        session
            .append_message(Message::user_text("appended"))
            .unwrap();
        let reverted = session.context_estimate();
        assert_eq!(reverted.provider_reported_tokens(), None);
        assert_eq!(reverted, session.estimate_context(&session.history()));
    }
}

// Covers: a source changing during a provider request must not be cached beside
// usage for the old immutable request, which would incorrectly calibrate new context.
// Owner: SDK accepted-request accounting; source transitions above own idle invalidation.
#[tokio::test]
async fn usage_is_bound_to_the_source_sampled_before_the_provider_request() {
    struct ChangingProvider {
        source: LiveContext,
        scripted: ScriptedProvider,
    }

    impl ModelProvider for ChangingProvider {
        fn identity(&self) -> ModelIdentity {
            self.scripted.identity()
        }

        fn send_turn<'a>(
            &'a self,
            request: crate::model::ModelRequest<'a>,
        ) -> crate::provider::ProviderFuture<'a> {
            self.scripted.send_turn(request)
        }

        fn send_turn_stream<'a>(
            &'a self,
            request: crate::model::ModelRequest<'a>,
            events: crate::provider::ProviderEventSender,
        ) -> crate::provider::ProviderFuture<'a> {
            Box::pin(async move {
                *self.source.0.lock().unwrap() = vec![Message::model_context("new context")];
                self.scripted.send_turn_stream(request, events).await
            })
        }
    }

    let source = LiveContext(Arc::new(Mutex::new(vec![Message::model_context(
        "old context",
    )])));
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [ScriptedTurn::streaming(
            vec![ModelEvent::Usage(ModelUsage {
                input_tokens: Some(1_000),
                ..ModelUsage::default()
            })],
            ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
        )],
    );
    let runtime = Rho::builder()
        .provider(ChangingProvider {
            source: source.clone(),
            scripted: provider.clone(),
        })
        .request_context(source.clone())
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::new()).await.unwrap();
    session.complete("request").await.unwrap();
    assert_eq!(
        provider.recorded_requests()[0].messages.last(),
        Some(&Message::model_context("old context"))
    );
    let estimate = session.context_estimate();
    assert_eq!(estimate.provider_reported_tokens(), None);
    assert_eq!(estimate, session.estimate_context(&session.history()));
}
