use super::{
    permission::PermissionDecision,
    scripted_agent::{Record, Script, ScriptedAgent, Signal, Step},
    session::run_on_channel,
    test_support::*,
    AcpSessionRequest,
};
use crate::{
    cli_runtime::parent_messages::{frame_parent_message, message_channel, ParentMessageSendError},
    run_artifacts::AttachmentEvent,
    subagent::RunState,
};
use agent_client_protocol::{
    schema::v1::{
        ContentChunk, PermissionOption, PermissionOptionKind, SessionUpdate, StopReason, ToolCall,
        ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
    },
    Channel, Error,
};
use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde_json::json;
use std::sync::{Arc, Mutex};

fn update(update: SessionUpdate) -> Step {
    Step::Update {
        update: Box::new(update),
    }
}
fn text(text: &str) -> Step {
    update(SessionUpdate::AgentMessageChunk(ContentChunk::new(
        text.to_owned().into(),
    )))
}
fn stop() -> Step {
    Step::Stop {
        reason: StopReason::EndTurn,
    }
}
fn script(turns: Vec<Vec<Step>>) -> Script {
    Script {
        turns,
        ..Script::default()
    }
}

async fn exercise(
    request: AcpSessionRequest,
    policy: TestPolicy,
    script: Script,
) -> Arc<Mutex<Record>> {
    let (client, agent) = Channel::duplex();
    let (fake, record, _signals) = ScriptedAgent::new(script);
    tokio::time::timeout(TEST_BUDGET, async {
        let (client, agent) =
            tokio::join!(run_on_channel(request, policy, client), fake.run(agent));
        client.unwrap();
        agent.unwrap();
    })
    .await
    .expect("ACP runtime/agent finished");
    record
}

// Covers: ACP success must preserve session metadata and final text, and write
// exactly one terminal event regardless of connection shutdown.
// Owner: runtime artifact boundary (not renderer layout/chrome).
#[tokio::test]
async fn successful_turn_has_one_terminal_write() {
    let dir = tempfile::tempdir().unwrap();
    let record = exercise(
        request(dir.path(), None),
        TestPolicy::new(dir.path()),
        script(vec![vec![text("answer"), stop()]]),
    )
    .await;
    let (status, events) = read_artifacts(dir.path());
    assert_eq!(
        (
            status.state,
            status.claude_session_id,
            status.result,
            status.turns,
            terminal_events(&events)
        ),
        (
            RunState::Ok,
            Some("scripted-session".into()),
            Some("answer".into()),
            1,
            vec![AttachmentEvent::Completed]
        )
    );
    let record = record.lock().unwrap();
    assert_eq!(
        record
            .requests
            .iter()
            .map(|request| request["method"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("initialize"),
            json!("session/new"),
            json!("session/prompt")
        ]
    );
    assert_eq!(
        record.requests[1],
        json!({"method":"session/new", "params":{"cwd":dir.path(),"mcpServers":[]}})
    );
    assert_eq!(
        record.requests[2],
        json!({"method":"session/prompt", "params":{"sessionId":"scripted-session", "prompt":[{"type":"text", "text":"task"}]}})
    );
}

// Covers: the chosen permission id reaches the agent, and Cursor-style completed
// rejection updates cannot masquerade as successful tool execution.
// Owner: ACP request/update loop; selection's security table is tested separately.
#[tokio::test]
async fn permission_reply_and_rejected_completion_follow_policy() {
    for (decision, id, status) in [
        (PermissionDecision::AllowOnce, "opaque-yes", ToolStatus::Ok),
        (PermissionDecision::Reject, "opaque-no", ToolStatus::Error),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut policy = TestPolicy::new(dir.path());
        policy.decision = decision;
        let call = ToolCall::new("shell-1", "run").kind(ToolKind::Execute);
        let permission = Step::Permission {
            call: Box::new(ToolCallUpdate::new(
                "shell-1",
                ToolCallUpdateFields::new().kind(ToolKind::Execute),
            )),
            options: vec![
                PermissionOption::new("opaque-yes", "yes", PermissionOptionKind::AllowOnce),
                PermissionOption::new("opaque-no", "no", PermissionOptionKind::RejectOnce),
            ],
        };
        let completed = ToolCallUpdate::new(
            "shell-1",
            ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
        );
        let record = exercise(
            request(dir.path(), None),
            policy,
            script(vec![vec![
                update(SessionUpdate::ToolCall(call)),
                permission,
                update(SessionUpdate::ToolCallUpdate(completed)),
                text("done"),
                stop(),
            ]]),
        )
        .await;
        assert_eq!(
            record.lock().unwrap().replies,
            vec![
                json!({"method":"session/request_permission", "result":{"outcome":{"outcome":"selected", "optionId":id}}})
            ]
        );
        let (run, events) = read_artifacts(dir.path());
        assert_eq!(run.state, RunState::Ok);
        let mut card = ToolCard::new(
            status,
            ToolFamily::FileCommand,
            ToolHeader::call("Execute", Some("run".into())),
        );
        if status == ToolStatus::Error {
            card.body = ToolBody::Lines(vec!["rejected by Rho permission policy".into()]);
            card.push_fact(ToolFact::Error {
                text: "rejected by Rho permission policy".into(),
            });
        }
        let finished = events
            .into_iter()
            .filter(|event| matches!(event, AttachmentEvent::ToolFinished { .. }))
            .collect::<Vec<_>>();
        assert_eq!(
            finished,
            vec![AttachmentEvent::ToolFinished {
                key: Some("shell-1".into()),
                presentation: card.into()
            }]
        );
    }
}

// Covers: a blocking non-underscore/no-sessionId extension receives either a
// policy reply or method_not_found, and neither path hangs the prompt turn.
// Owner: ACP catch-all request routing, not Cursor extension policy.
#[tokio::test]
async fn extension_requests_are_always_answered() {
    let dir = tempfile::tempdir().unwrap();
    let record = exercise(
        request(dir.path(), None),
        TestPolicy::new(dir.path()),
        script(vec![vec![
            Step::Extension {
                method: "fake/ask".into(),
                params: json!({"question":"q"}),
            },
            Step::Extension {
                method: "unknown/method".into(),
                params: json!({}),
            },
            text("continued"),
            stop(),
        ]]),
    )
    .await;
    assert_eq!(
        record.lock().unwrap().replies,
        vec![
            json!({"method":"fake/ask", "result":{"answer":"continue"}}),
            json!({"method":"unknown/method", "error":Error::method_not_found()})
        ]
    );
    let (status, events) = read_artifacts(dir.path());
    assert_eq!(
        (status.state, status.result, terminal_events(&events)),
        (
            RunState::Ok,
            Some("continued".into()),
            vec![AttachmentEvent::Completed]
        )
    );
}

// Covers: parent corrections use a second turn in the same session, are
// journaled only after prompt delivery, and a sealed run rejects late sends.
// Owner: ACP turn scheduling; in-flight seal semantics live with the queue.
#[tokio::test]
async fn queued_parent_message_becomes_next_prompt_and_port_seals() {
    let dir = tempfile::tempdir().unwrap();
    let (handle, inbox) = message_channel();
    handle.send("course correction".into()).unwrap();
    let record = exercise(
        request(dir.path(), Some(inbox)),
        TestPolicy::new(dir.path()),
        script(vec![
            vec![text("first"), stop()],
            vec![text("corrected"), stop()],
        ]),
    )
    .await;
    assert_eq!(
        handle.send("late".into()),
        Err(ParentMessageSendError::Closed)
    );
    let prompts = record
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|request| request["method"] == "session/prompt")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        prompts,
        vec![
            json!({"method":"session/prompt", "params":{"sessionId":"scripted-session", "prompt":[{"type":"text", "text":"task"}]}}),
            json!({"method":"session/prompt", "params":{"sessionId":"scripted-session", "prompt":[{"type":"text", "text":frame_parent_message("course correction")}]}})
        ]
    );
    let (status, events) = read_artifacts(dir.path());
    assert_eq!(
        (
            status.state,
            status.result,
            status.turns,
            terminal_events(&events)
        ),
        (
            RunState::Ok,
            Some("corrected".into()),
            2,
            vec![AttachmentEvent::Completed]
        )
    );
    let messages = events
        .into_iter()
        .filter(|event| matches!(event, AttachmentEvent::Message(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        messages,
        vec![AttachmentEvent::Message(Box::new(
            crate::presentation::parent_message_card(
                "course correction".into(),
                crate::presentation::NotificationDelivery::Queued,
                "sent as the next agent turn".into()
            )
        ))]
    );
}

// Covers: cancel mid-turn sends session/cancel and waits for the cancelled
// prompt response, without turning it into success or writing two terminals.
// Owner: ACP lifecycle. The fake signals readiness rather than sleeping.
#[tokio::test]
async fn cancellation_is_a_protocol_notification_and_stopped_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let request = request(dir.path(), None);
    let cancellation = request.cancellation.clone();
    let (client, agent) = Channel::duplex();
    let (fake, record, mut signals) =
        ScriptedAgent::new(script(vec![vec![text("partial"), Step::Hang]]));
    tokio::time::timeout(TEST_BUDGET, async {
        let cancel = async {
            loop {
                match signals
                    .recv()
                    .await
                    .expect("fake remains connected until waiting")
                {
                    Signal::Waiting => break,
                    Signal::Prompt | Signal::Cancel => {}
                }
            }
            cancellation.cancel();
        };
        let (client, agent, ()) = tokio::join!(
            run_on_channel(request, TestPolicy::new(dir.path()), client),
            fake.run(agent),
            cancel
        );
        client.unwrap();
        agent.unwrap();
    })
    .await
    .expect("cancelled run finished");
    let cancellations = record
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|request| request["method"] == "session/cancel")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        cancellations,
        vec![json!({"method":"session/cancel", "params":{"sessionId":"scripted-session"}})]
    );
    let (status, events) = read_artifacts(dir.path());
    assert_eq!(
        (status.state, status.result, terminal_events(&events)),
        (
            RunState::Stopped,
            Some("partial".into()),
            vec![AttachmentEvent::Cancelled]
        )
    );
}

// Covers: setup failures name the wire step, unavailable modes never prompt,
// and EOF mid-turn fails instead of hanging or inventing a stop reason.
// Owner: ACP runtime error boundaries. Only required diagnostic context is checked.
#[tokio::test]
async fn setup_and_connection_failures_are_terminal_errors() {
    let cases = [
        (
            Script {
                new_session_error: Some("authRequired: login required".into()),
                ..Script::default()
            },
            None,
            None,
            "session/new",
            "authRequired",
            false,
        ),
        (
            Script::default(),
            Some("plan"),
            None,
            "session/set_mode",
            "plan",
            false,
        ),
        (
            Script {
                modes: vec!["plan".into()],
                set_mode_error: Some("mode denied".into()),
                ..Script::default()
            },
            Some("plan"),
            None,
            "session/set_mode",
            "mode denied",
            false,
        ),
        (
            Script {
                initialize_error: Some("bad version".into()),
                ..Script::default()
            },
            None,
            None,
            "initialize",
            "bad version",
            false,
        ),
        (
            Script {
                authenticate_error: Some("bad auth".into()),
                ..Script::default()
            },
            None,
            Some("login"),
            "authenticate",
            "bad auth",
            false,
        ),
        (
            script(vec![vec![Step::Disconnect]]),
            None,
            None,
            "acp",
            "closed",
            true,
        ),
    ];
    for (script, mode, auth, step, detail, prompted) in cases {
        let dir = tempfile::tempdir().unwrap();
        let mut policy = TestPolicy::new(dir.path());
        policy.mode = mode.map(str::to_owned);
        policy.auth = auth.map(str::to_owned);
        let record = exercise(request(dir.path(), None), policy, script).await;
        let (status, events) = read_artifacts(dir.path());
        assert_eq!(status.state, RunState::Error);
        let error = status.error.unwrap();
        assert!(
            error.contains(step) && error.contains(detail),
            "required ACP diagnostic context: {error}"
        );
        assert_eq!(
            terminal_events(&events),
            vec![AttachmentEvent::Failed(error)]
        );
        assert_eq!(
            record
                .lock()
                .unwrap()
                .requests
                .iter()
                .any(|request| request["method"] == "session/prompt"),
            prompted
        );
    }
}
