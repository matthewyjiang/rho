use super::{converse, OneShotAnswer, OneShotError, OneShotProgress};
use crate::acp_runtime::{
    permission::PermissionDecision,
    scripted_agent::{Script, ScriptedAgent, Signal, Step},
    test_support::{TestPolicy, TEST_BUDGET},
};
use agent_client_protocol::{
    schema::v1::{
        ContentChunk, Cost, PermissionOption, PermissionOptionKind, SessionUpdate, StopReason,
        ToolCallUpdate, ToolCallUpdateFields, UsageUpdate,
    },
    Channel,
};
use pretty_assertions::assert_eq;
use rho_sdk::CancellationToken;
use serde_json::json;

fn chunk(text: &str) -> SessionUpdate {
    SessionUpdate::AgentMessageChunk(ContentChunk::new(text.to_owned().into()))
}

fn update(update: SessionUpdate) -> Step {
    Step::Update {
        update: Box::new(update),
    }
}

/// Owned copy of a progress event, for comparing the sequence.
#[derive(Debug, PartialEq)]
enum Seen {
    Thinking,
    Answer(String),
}

// Covers: a one-shot streams thinking and the growing answer, keeps the
// reported cost, answers permission requests by policy, and returns the full
// answer at end_turn. Owner: ACP one-shot client; no run artifacts exist.
#[tokio::test]
async fn one_shot_streams_the_answer_and_reports_cost() {
    let dir = tempfile::tempdir().unwrap();
    let mut policy = TestPolicy::new(dir.path());
    policy.decision = PermissionDecision::Reject;
    let (client, agent) = Channel::duplex();
    let (fake, record, _signals) = ScriptedAgent::new(Script {
        turns: vec![vec![
            update(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                "hmm".to_owned().into(),
            ))),
            Step::Permission {
                call: Box::new(ToolCallUpdate::new("call-1", ToolCallUpdateFields::new())),
                options: vec![
                    PermissionOption::new("yes", "Yes", PermissionOptionKind::AllowOnce),
                    PermissionOption::new("no", "No", PermissionOptionKind::RejectOnce),
                ],
            },
            update(chunk("look ")),
            update(chunk("closer")),
            update(SessionUpdate::UsageUpdate(
                UsageUpdate::new(10, 100).cost(Cost::new(0.25, "USD")),
            )),
            Step::Stop {
                reason: StopReason::EndTurn,
            },
        ]],
        ..Script::default()
    });
    let mut seen = Vec::new();
    let cancellation = CancellationToken::new();
    let (answer, agent) = tokio::time::timeout(TEST_BUDGET, async {
        tokio::join!(
            converse(
                client,
                policy,
                "review this",
                dir.path(),
                &cancellation,
                |progress| seen.push(match progress {
                    OneShotProgress::Thinking => Seen::Thinking,
                    OneShotProgress::Answer(text) => Seen::Answer(text.to_owned()),
                }),
            ),
            fake.run(agent)
        )
    })
    .await
    .expect("one-shot finished");
    agent.unwrap();
    assert_eq!(
        answer,
        Ok(OneShotAnswer {
            text: "look closer".into(),
            cost_usd: Some(0.25),
        })
    );
    assert_eq!(
        seen,
        vec![
            Seen::Thinking,
            Seen::Answer("look ".into()),
            Seen::Answer("look closer".into()),
        ]
    );
    let record = record.lock().unwrap();
    assert_eq!(
        record.replies,
        vec![
            json!({"method":"session/request_permission", "result":{"outcome":{"outcome":"selected", "optionId":"no"}}})
        ]
    );
}

// Covers: cancelling while the agent works sends session/cancel and returns
// Cancelled instead of a partial answer. Owner: ACP one-shot client. The fake
// signals readiness rather than sleeping.
#[tokio::test]
async fn cancelled_one_shot_sends_session_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let (client, agent) = Channel::duplex();
    let (fake, record, mut signals) = ScriptedAgent::new(Script {
        turns: vec![vec![update(chunk("partial")), Step::Hang]],
        ..Script::default()
    });
    let cancellation = CancellationToken::new();
    let cancel = async {
        while let Some(signal) = signals.recv().await {
            if signal == Signal::Waiting {
                cancellation.cancel();
                break;
            }
        }
    };
    let (answer, agent, ()) = tokio::time::timeout(TEST_BUDGET, async {
        tokio::join!(
            converse(
                client,
                TestPolicy::new(dir.path()),
                "review this",
                dir.path(),
                &cancellation,
                |_| {},
            ),
            fake.run(agent),
            cancel
        )
    })
    .await
    .expect("cancelled one-shot finished");
    agent.unwrap();
    assert_eq!(answer, Err(OneShotError::Cancelled));
    assert!(record
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|request| request["method"] == "session/cancel"));
}
