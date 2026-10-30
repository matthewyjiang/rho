use std::num::{NonZeroU64, NonZeroUsize};

use pretty_assertions::assert_eq;

use crate::{model::Message, CompactionOutput, CompactionPolicy, CompactionRequest, Compactor};

// Covers: each explicit policy compacts exactly at its own threshold and
// ignores the other measure.
// Owner: SDK compaction policy
#[test]
fn policy_uses_explicit_message_threshold() {
    let by_messages = CompactionPolicy::after_messages(NonZeroUsize::new(3).unwrap());
    let by_tokens = CompactionPolicy::at_context_tokens(NonZeroU64::new(1_000).unwrap());
    let cases = [
        ("messages below", &by_messages, 2, u64::MAX, false),
        ("messages at", &by_messages, 3, 0, true),
        ("tokens below", &by_tokens, usize::MAX, 999, false),
        ("tokens at", &by_tokens, 0, 1_000, true),
    ];

    for (case, policy, messages, tokens, expected) in cases {
        assert_eq!(policy.should_compact(messages, tokens), expected, "{case}");
    }
}

#[test]
fn replacement_history_must_not_be_empty() {
    assert!(CompactionOutput::new(Vec::new()).is_err());
}

#[test]
fn compaction_state_tracks_token_and_cost_accounting() {
    use crate::{model::ModelUsage, CompactionState, Revision};

    let mut state = CompactionState::default();
    state.record(4, 1_200, 300, Some(2_500), Revision::from_u64(3));

    assert_eq!(state.completed_compactions(), 1);
    assert_eq!(state.removed_messages(), 4);
    assert_eq!(state.removed_tokens(), 900);
    assert_eq!(state.removed_cost_usd_micros(), 2_500);
    assert_eq!(state.last_previous_tokens(), Some(1_200));
    assert_eq!(state.last_current_tokens(), Some(300));

    let restored = CompactionState::from_parts(1, 4, Some(Revision::from_u64(3)))
        .with_removed_tokens(900)
        .with_removed_cost_usd_micros(2_500)
        .with_last_compaction_tokens(1_200, 300);
    assert_eq!(restored, state);

    let usage = ModelUsage {
        cost_usd_micros: Some(100),
        ..ModelUsage::default()
    };
    let output =
        CompactionOutput::with_usage(vec![Message::System("summary".into())], usage).unwrap();
    assert_eq!(output.usage().cost_usd_micros, Some(100));
}

// Covers: a compactor cannot reproduce the session's cached request prefix
// because manual or automatic requests lose the prompt cache key or the tool
// specs the session's provider turns advertise, or a manual request drops the
// caller's instructions, fails to normalize them, or automatic compaction
// invents some.
// Owner: SDK compaction contract
#[tokio::test]
async fn compaction_requests_carry_session_cache_key_tool_specs_and_instructions() {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use crate::{
        model::{ContentBlock, ModelIdentity, ModelResponse, ToolSpec},
        provider::{ScriptedProvider, ScriptedTurn},
        tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput},
        CompactionFuture, Rho, SessionOptions,
    };

    #[derive(Clone, Default)]
    struct Recording(Arc<Mutex<Vec<CompactionRequest>>>);
    impl Compactor for Recording {
        fn compact<'a>(&'a self, request: CompactionRequest) -> CompactionFuture<'a> {
            self.0.lock().unwrap().push(request);
            Box::pin(async { CompactionOutput::new(vec![Message::user_text("summary")]) })
        }
    }

    let spec = ToolSpec {
        name: "read".into(),
        description: "read".into(),
        input_schema: json!({"type": "object"}),
    };
    let compactor = Recording::default();
    let runtime = Rho::builder()
        .provider(ScriptedProvider::new(
            ModelIdentity::new("scripted", "test", "compaction"),
            [ScriptedTurn::completed(ModelResponse::Assistant(vec![
                ContentBlock::Text("done".into()),
            ]))],
        ))
        .tool(ScriptedTool::new(
            spec.clone(),
            ScriptedToolOutcome::Success(ToolOutput::text("ok")),
        ))
        .compactor(compactor.clone())
        .compaction_policy(CompactionPolicy::after_messages(
            NonZeroUsize::new(1).unwrap(),
        ))
        .build()
        .unwrap();
    let session = runtime
        .session(
            SessionOptions::new()
                .history(vec![Message::user_text("old")])
                .prompt_cache_key("rho:session"),
        )
        .await
        .unwrap();

    let guidance = [
        ("keep the plan", Some("keep the plan")),
        (" \nkeep the plan\t ", Some("keep the plan")),
        ("", None),
        (" \n\t ", None),
    ];
    for (instructions, expected) in guidance {
        let request = CompactionRequest::new(Vec::new(), crate::CancellationToken::new())
            .with_instructions(instructions);
        assert_eq!(request.instructions(), expected, "{instructions:?}");
        session
            .compact_with_instructions(instructions)
            .await
            .unwrap();
    }
    session.compact().await.unwrap();
    session.complete("next").await.unwrap();

    let requests = compactor.0.lock().unwrap();
    let carried = requests
        .iter()
        .map(|request| {
            (
                request.trigger(),
                request.prompt_cache_key().map(str::to_owned),
                request.tool_specs().map(<[ToolSpec]>::to_vec),
                request.instructions().map(str::to_owned),
            )
        })
        .collect::<Vec<_>>();
    let expected = guidance
        .into_iter()
        .map(|(_, instructions)| {
            (
                crate::CompactionTrigger::Manual,
                instructions.map(str::to_owned),
            )
        })
        .chain([
            (crate::CompactionTrigger::Manual, None),
            (crate::CompactionTrigger::Automatic, None),
        ])
        .map(|(trigger, instructions)| {
            (
                trigger,
                Some("rho:session".to_owned()),
                Some(vec![spec.clone()]),
                instructions,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(carried, expected);
}
