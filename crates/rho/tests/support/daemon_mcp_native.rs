//! The native `sessions` tool's control actions through the real binary
//! (#1714): an agent in a daemon session starts a session in another
//! workspace, follows it, cancels it, finds it waiting for a person, and
//! answers its questions, under the same core, and so the same answers, as
//! `rho mcp serve`.
//!
//! Each step is one turn of the calling session: the `tool-call` fixture
//! makes the `sessions` call its prompt spells out, and its answer is the
//! tool's result. An MCP host stands in for whoever watches that session.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{JobList, SessionAnswer, SessionAnswerQuestion},
    Client, ErrorCode, IdleWake, InputAnswer, InputRequestId, JobListParams, JobState, JobSummary,
    QuestionAnswer, Resolution, SessionAnswerParams, SessionAnswerQuestionParams, SessionEvent,
    SessionId, SessionSetup,
};
use serde_json::{json, Value};

use super::{bash, call, elsewhere, stdio_server, McpClient, Sandbox, BLOCKED, DEADLINE};
use crate::sessions::{Follower, REPLY};

/// A `tool-call` fixture prompt that calls the native `sessions` tool with
/// `arguments`.
fn native(arguments: Value) -> Value {
    json!({"id": "sessions", "name": "sessions", "arguments": arguments})
}

/// Waits for `session`'s turn to stop, through `host`, and returns the
/// last `wait`.
async fn stopped(host: &McpClient, session: &SessionId) -> Value {
    let started = std::time::Instant::now();
    let mut cursor = Value::Null;
    loop {
        let mut arguments = json!({"session": session, "timeout_secs": 50});
        if cursor.is_string() {
            arguments["cursor"] = cursor.clone();
        }
        let waited = call(host, "wait", arguments).await.unwrap();
        if waited["done"] == json!(true) {
            return waited;
        }
        assert!(started.elapsed() < DEADLINE, "{session} still runs");
        cursor = waited["cursor"].clone();
    }
}

/// The final answer of `session`'s turn, once it ended.
async fn answer(host: &McpClient, session: &SessionId) -> String {
    let waited = stopped(host, session).await;
    match waited["outcome"]["text"].as_str() {
        Some(text) => text.to_owned(),
        None => panic!("{session} ended without an answer: {waited}"),
    }
}

/// Runs one turn of `agent` that makes the native `sessions` call
/// `arguments`, and returns what the tool answered: its JSON result, or the
/// error the model read.
async fn ask(
    sandbox: &Sandbox,
    client: &Client,
    host: &McpClient,
    agent: &SessionId,
    arguments: Value,
) -> Result<Value, String> {
    sandbox
        .prompt_tool_call(client, agent, &native(arguments))
        .await;
    let text = answer(host, agent).await;
    serde_json::from_str(&text).map_err(|_| text)
}

/// The job of `session`'s turns.
async fn job_of(client: &Client, session: &SessionId) -> JobSummary {
    client
        .request::<JobList>(&JobListParams {})
        .await
        .unwrap()
        .jobs
        .into_iter()
        .find(|job| job.session.as_ref() == Some(session))
        .expect("the daemon lists the session's job")
}

/// The session a `new` result names.
fn started(result: &Result<Value, String>) -> SessionId {
    match result.as_ref().map(|result| result["session"].as_str()) {
        Ok(Some(session)) => SessionId::new(session),
        _ => panic!("a `new` result, not {result:?}"),
    }
}

// Covers: from a daemon session, an agent starts a session in another
// workspace with the native tool, waits for its turn, and reads its final
// answer; the started session's job is a child of the agent's, which `list`
// names as its parent (#1714).
// Owner: native sessions tool over the session-control core.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_starts_a_session_elsewhere_and_reads_its_answer() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let workspace = elsewhere(&sandbox);
    let client = sandbox.start_daemon("tool-call").await;
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    let agent = sandbox.new_session(&client).await;

    let new = ask(
        &sandbox,
        &client,
        &host,
        &agent,
        json!({"action": "new", "workspace": workspace, "prompt": "hello"}),
    )
    .await;
    let child = started(&new);
    let waited = ask(
        &sandbox,
        &client,
        &host,
        &agent,
        json!({"action": "wait", "session": child, "timeout_secs": 50}),
    )
    .await
    .unwrap();
    let read = ask(
        &sandbox,
        &client,
        &host,
        &agent,
        json!({"action": "read", "session": child}),
    )
    .await
    .unwrap();
    let listed = call(&host, "list", json!({"workspace": workspace}))
        .await
        .unwrap();
    let (agent_job, child_job) = (job_of(&client, &agent).await, job_of(&client, &child).await);

    assert_eq!(
        (
            new.unwrap()["workspace"].clone(),
            waited["state"].clone(),
            waited["done"].clone(),
            waited["outcome"]["text"].clone(),
            read["text"]
                .as_str()
                .is_some_and(|text| text.ends_with(REPLY)),
            listed["sessions"][0]["parent"].clone(),
            child_job.parent,
            child_job.workspace,
        ),
        (
            json!(workspace),
            json!("completed"),
            json!(true),
            json!(REPLY),
            true,
            json!(agent),
            Some(agent_job.id),
            workspace,
        ),
        "read: {read}\nlog:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: the native tool and `rho mcp serve` answer the same calls the same
// way, results and refusals alike, since both adapt one core (#1714).
// Owner: native sessions tool and rho mcp serve, one session-control core.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_native_tool_and_the_mcp_server_answer_alike() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let workspace = elsewhere(&sandbox);
    let client = sandbox.start_daemon("tool-call").await;
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    let agent = sandbox.new_session(&client).await;
    let child = started(
        &call(
            &host,
            "new",
            json!({"workspace": workspace, "prompt": "hello"}),
        )
        .await,
    );
    stopped(&host, &child).await;

    let calls = [
        json!({"action": "list", "workspace": workspace}),
        json!({"action": "read", "session": child, "start": 3, "chars": 40}),
        json!({"action": "wait", "session": child, "timeout_secs": 0}),
        json!({"action": "cancel", "session": "no-such-session"}),
        json!({"action": "prompt", "session": child, "text": " "}),
        json!({"action": "new", "workspace": "relative", "prompt": "hello"}),
        json!({"action": "list", "limit": 1001}),
    ];
    let mut native_answers = Vec::new();
    let mut mcp_answers = Vec::new();
    for arguments in calls {
        native_answers.push(ask(&sandbox, &client, &host, &agent, arguments.clone()).await);
        let mut arguments = arguments.as_object().unwrap().clone();
        let action = arguments.remove("action").unwrap();
        mcp_answers.push(call(&host, action.as_str().unwrap(), Value::Object(arguments)).await);
    }

    assert_eq!(native_answers, mcp_answers);
}

// Covers: `cancel` from the native tool ends a started session's running
// turn as `cancelled`, which the agent's next `wait` reports (#1714).
// Owner: native sessions tool over the session-control core.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_cancels_a_session_it_started() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let workspace = elsewhere(&sandbox);
    let client = sandbox.start_daemon("tool-call").await;
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    let agent = sandbox.new_session(&client).await;
    let ask = async |arguments| ask(&sandbox, &client, &host, &agent, arguments).await;

    let child = started(
        &ask(json!({"action": "new", "workspace": workspace, "prompt": bash(BLOCKED)})).await,
    );
    let underway = ask(json!({"action": "wait", "session": child, "timeout_secs": 0}))
        .await
        .unwrap();
    let cancelled = ask(json!({"action": "cancel", "session": child})).await;
    let waited = ask(json!({"action": "wait", "session": child, "timeout_secs": 50}))
        .await
        .unwrap();

    assert_eq!(
        (
            // Queued for the shared budget, or already running.
            ["queued", "running"].contains(&underway["state"].as_str().unwrap_or_default()),
            cancelled,
            waited["state"].clone(),
            waited["done"].clone(),
            waited["outcome"].clone(),
        ),
        (
            true,
            Ok(json!({})),
            json!("cancelled"),
            json!(true),
            json!({"type": "cancelled"}),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

/// Runs one turn of `agent` that makes the native `sessions` call
/// `arguments`, which asks a person first; approves it once, and returns
/// what it asked for and what the tool answered.
async fn ask_approved(
    sandbox: &Sandbox,
    client: &Client,
    host: &McpClient,
    agent: &SessionId,
    arguments: Value,
) -> (Value, Result<Value, String>) {
    sandbox
        .prompt_tool_call(client, agent, &native(arguments))
        .await;
    let asked = stopped(host, agent).await;
    let request: InputRequestId = serde_json::from_value(asked["inputs"][0]["id"].clone())
        .unwrap_or_else(|_| panic!("the call to ask first, not {asked}"));
    client
        .request::<SessionAnswer>(&SessionAnswerParams {
            session: agent.clone(),
            request,
            answer: InputAnswer::AllowOnce,
        })
        .await
        .unwrap();
    let answered = sandbox
        .poll_until("the approved call to end", async || {
            let waited = stopped(host, agent).await;
            let text = waited["outcome"]["text"].as_str()?;
            Some(serde_json::from_str::<Value>(text).map_err(|_| text.to_owned()))
        })
        .await;
    (asked["inputs"][0]["ask"]["operation"].clone(), answered)
}

// Covers: acting on a session asks for approval naming the action, the
// target's workspace, and the prompt sent, and watching asks for the read
// of the session store that searching history asks for; a session the agent
// starts that needs approval reports `waiting_for_input` to the agent's
// `wait` and stays waiting for a person (#1714).
// Owner: native sessions tool, session-control capability, core policy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_started_session_that_asks_waits_for_a_person() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("supervised");
    let workspace = elsewhere(&sandbox);
    let client = sandbox.start_daemon("tool-call").await;
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    let agent = sandbox.new_session(&client).await;
    let write = json!({
        "id": "edit",
        "name": "write",
        "arguments": {"path": "note.txt", "content": "approved"},
    })
    .to_string();

    // The person approves the agent's start and its wait; nobody approves
    // the started session's write.
    let (start_asked, new) = ask_approved(
        &sandbox,
        &client,
        &host,
        &agent,
        json!({"action": "new", "workspace": workspace, "prompt": write}),
    )
    .await;
    let child = started(&new);
    let (wait_asked, waited) = ask_approved(
        &sandbox,
        &client,
        &host,
        &agent,
        json!({"action": "wait", "session": child, "timeout_secs": 50}),
    )
    .await;
    let waited = waited.unwrap();

    assert_eq!(
        (
            start_asked,
            wait_asked["kind"].clone(),
            wait_asked["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("/sessions")),
            waited["state"].clone(),
            waited["inputs"][0]["ask"]["kind"].clone(),
            job_of(&client, &child).await.state,
            workspace.join("note.txt").exists(),
        ),
        (
            json!({
                "kind": "control_session",
                "action": "new",
                "workspace": workspace,
                "text": write,
            }),
            json!("read_path"),
            true,
            json!("waiting_for_input"),
            json!("approval"),
            JobState::WaitingForInput,
            false,
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a session's question, with the reason its agent gave, reaches the
// session that started it through `wait` and an attached client alike; that
// session's `answer` hands the asker's tool the answers as a questionnaire
// result whose source says a session answered; clients see the request
// closed by a session; a session that did not start the asker is refused by the tool and
// by the daemon alike; a second answer finds the question closed (#1714).
// Owner: session-control answer, daemon `rho/session/answer_question`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_that_started_another_answers_its_question() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let workspace = elsewhere(&sandbox);
    let client = sandbox.start_daemon("tool-call").await;
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    let agent = sandbox.new_session(&client).await;
    let outsider = sandbox.new_session(&client).await;
    let question = json!({
        "id": "ask",
        "name": "questionnaire",
        "arguments": {
            "reason": "The logo needs one.",
            "questions": [{"id": "color", "question": "Which color?", "choices": ["Red", "Blue"]}],
        },
    })
    .to_string();
    let worker = started(
        &ask(
            &sandbox,
            &client,
            &host,
            &agent,
            json!({"action": "new", "workspace": workspace, "prompt": question}),
        )
        .await,
    );
    let asked = stopped(&host, &worker).await;
    let request: InputRequestId = serde_json::from_value(asked["inputs"][0]["id"].clone())
        .unwrap_or_else(|_| panic!("the worker to ask, not {asked}"));
    let (mut watcher, attached) = Follower::attach(&sandbox, &worker, Vec::new(), None).await;
    let asks = (
        asked["inputs"][0]["ask"].clone(),
        attached
            .inputs
            .first()
            .map(|input| serde_json::to_value(&input.ask).unwrap()),
    );
    let answer_with = |answers| json!({"action": "answer", "session": worker, "request": request, "answers": answers});

    let outsiders = ask(
        &sandbox,
        &client,
        &host,
        &outsider,
        answer_with(json!({"color": ["Red"]})),
    )
    .await;
    let declared = client
        .request::<SessionAnswerQuestion>(&SessionAnswerQuestionParams {
            session: worker.clone(),
            request,
            answer: QuestionAnswer::Decline,
            by: outsider.clone(),
        })
        .await
        .map_err(|error| error.error_code());
    let answered = ask(
        &sandbox,
        &client,
        &host,
        &agent,
        answer_with(json!({"color": ["Blue"]})),
    )
    .await;
    watcher
        .read_until(
            |event| matches!(event, SessionEvent::InputResolved { id, .. } if *id == request),
        )
        .await;
    let again = client
        .request::<SessionAnswerQuestion>(&SessionAnswerQuestionParams {
            session: worker.clone(),
            request,
            answer: QuestionAnswer::Decline,
            by: agent.clone(),
        })
        .await
        .map_err(|error| error.error_code());
    let result: Value = serde_json::from_str(&answer(&host, &worker).await).unwrap();
    let closed = watcher.events.last().map(|(_, event)| event.clone());

    let ask = json!({
        "kind": "question",
        "title": "questionnaire",
        "reason": "The logo needs one.",
        "questions": [{
            "id": "color",
            "prompt": "Which color?",
            "choices": [
                {"value": "Red", "label": "Red"},
                {"value": "Blue", "label": "Blue"},
            ],
            "selection": "one",
            "allow_other": false,
            "required": true,
        }],
    });
    assert_eq!(
        (
            asks,
            outsiders.map_err(drop),
            declared,
            answered,
            closed,
            again,
            result
        ),
        (
            (ask.clone(), Some(ask)),
            // The tool hands the model the daemon's refusal.
            Err(()),
            Err(Some(ErrorCode::InvalidParams)),
            Ok(json!({})),
            Some(SessionEvent::InputResolved {
                id: request,
                resolution: Resolution::AnsweredBySession,
            }),
            Err(Some(ErrorCode::InputNotOpen)),
            json!({"answers": [{"id": "color", "answer": "Blue"}], "source": "session"}),
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: an agent at the nesting cap's depth cannot start another session;
// the native tool's refusal names the budget, its limit, and the depth
// asked for, as the MCP server's does (#1714).
// Owner: session-control core depth cap, through the native tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starting_past_the_nesting_cap_is_refused() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let workspace = elsewhere(&sandbox);
    // Sessions it starts run at depth 3, the default cap.
    let host = stdio_server(&sandbox, "tool-call", &[], &[("RHO_SESSION_DEPTH", "2")]).await;
    let start = native(json!({"action": "new", "workspace": workspace, "prompt": "hello"}));

    let agent = started(
        &call(
            &host,
            "new",
            json!({"workspace": workspace, "prompt": start.to_string()}),
        )
        .await,
    );

    assert_eq!(
        answer(&host, &agent).await,
        "session depth budget: limit 3, asked 4; a session at depth 3 may not start or prompt another"
    );
}

// Covers: the native tool caps a session's running sessions at
// `behavior.agent_concurrency`, live: at 1, a second `new` while the first
// runs is refused naming the budget, its limit, and the count asked for; once
// the config says 32, the next `new` starts.
// Owner: native sessions tool's session cap, from the daemon's agent budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_cap_follows_agent_concurrency() {
    let sandbox = Sandbox::new();
    sandbox.set_config("bypass", "agent_concurrency = 1\n");
    let workspace = elsewhere(&sandbox);
    let client = sandbox.start_daemon("tool-call").await;
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    let agent = sandbox.new_session(&client).await;
    let ask = async |arguments| ask(&sandbox, &client, &host, &agent, arguments).await;
    let start = json!({"action": "new", "workspace": workspace, "prompt": bash(BLOCKED)});

    let first = ask(start.clone()).await;
    let refused = ask(start.clone()).await;
    // Read at the agent's next turn start, which publishes it.
    sandbox.set_config("bypass", "agent_concurrency = 32\n");
    let raised = ask(start).await;
    std::fs::write(workspace.join("go"), "").unwrap();

    assert_eq!(
        (first.is_ok(), refused, raised.is_ok()),
        (
            true,
            Err(format!(
                "concurrent sessions budget for session {agent}: limit 1, asked 2"
            )),
            true,
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: in a rho that runs its session itself, with no daemon running, a
// watching call reports that no daemon runs; a tool call never starts the
// long-lived daemon on its own (#1714).
// Owner: native sessions tool's daemon reach.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watching_without_a_daemon_starts_none() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let mut run = tokio::process::Command::from(sandbox.rho("tool-call"));
    run.args(["run", &native(json!({"action": "list"})).to_string()])
        .stdout(std::process::Stdio::piped());

    let output = tokio::time::timeout(DEADLINE, run.output())
        .await
        .expect("the run ends")
        .unwrap();

    assert_eq!(
        (
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            sandbox.isolated.daemon_socket(),
        ),
        (
            "could not reach the rho daemon: rho daemon is not running; see \
             https://matthewyjiang.github.io/rho/daemon"
                .to_owned(),
            None,
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

/// The texts of the boundary inputs `session` saved, in order.
fn boundary_inputs(sandbox: &Sandbox, session: &SessionId) -> Vec<String> {
    sandbox
        .session_records(session)
        .iter()
        .flat_map(|record| {
            record["transition"]["delta"]["appended_history"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .flat_map(|message| {
            message["BoundaryInput"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|block| block["Text"].as_str().map(str::to_owned))
        .collect()
}

// Covers: an agent in a daemon session that no client follows, and whose
// client asked for daemon turns, starts a session elsewhere and ends its own
// turn; once the started session's turn ends, the daemon wakes the agent with
// one turn whose one notification names that session and quotes its answer,
// so the agent never has to poll `sessions wait`.
// Owner: daemon wake for started sessions, through the native tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_started_sessions_end_wakes_its_idle_starter() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let workspace = elsewhere(&sandbox);
    let client = sandbox.start_daemon("tool-call").await;
    let host = stdio_server(&sandbox, "tool-call", &[], &[]).await;
    // Opened and prompted, never attached: no client follows the agent.
    let agent = sandbox
        .new_session_with_setup(
            &client,
            SessionSetup {
                idle_wake: IdleWake::Turn,
                ..SessionSetup::default()
            },
        )
        .await;
    let start = native(json!({"action": "new", "workspace": workspace, "prompt": bash(BLOCKED)}));

    sandbox.prompt_tool_call(&client, &agent, &start).await;
    let agent_job = job_of(&client, &agent).await.id;
    // The agent's turn ended while the session it started still runs.
    let child = sandbox
        .poll_until("the agent's turn to end", async || {
            let jobs = client.request::<JobList>(&JobListParams {}).await.unwrap();
            let agent_ended = jobs
                .jobs
                .iter()
                .any(|job| job.id == agent_job && job.state == JobState::Completed);
            let child = jobs
                .jobs
                .into_iter()
                .find(|job| job.parent.as_ref() == Some(&agent_job))?;
            agent_ended.then_some(child.session?)
        })
        .await;
    std::fs::write(workspace.join("go"), "").unwrap();
    let child_answer = answer(&host, &child).await;
    // Settled again: the turn the daemon started took the notice. The
    // fixture answers a turn with no prompt with its last tool result, so
    // the turn's own answer says nothing here.
    stopped(&host, &agent).await;
    let notices: Vec<String> = boundary_inputs(&sandbox, &agent)
        .into_iter()
        .filter(|text| text.lines().next() == Some("[session notification]"))
        .collect();
    let names = |text: &String| (text.contains(child.as_str()), text.contains(&child_answer));

    assert_eq!(
        notices.iter().map(names).collect::<Vec<_>>(),
        vec![(true, true)],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
