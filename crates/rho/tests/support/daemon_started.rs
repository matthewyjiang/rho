//! Sessions another session starts (#1714): the daemon records the job
//! writing the starting session as the new session's parent, and the new
//! session's turns wait for capacity in the daemon-wide agent budget, as
//! subagent runs do.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{JobList, SessionCancel, SessionNew},
    Budget, Client, JobListParams, JobState, JobSummary, QueuePlace, SessionCancelParams,
    SessionId, SessionNewParams, SessionSetup,
};

use super::sessions::{settled_job, Sandbox};

/// A `tool-call` fixture prompt that runs bash until `go` exists in its
/// workspace.
fn blocked() -> serde_json::Value {
    serde_json::json!({
        "id": "bash",
        "name": "bash",
        "arguments": {"command": "while [ ! -e go ]; do sleep 0.02; done"},
    })
}

/// Opens a session in the workspace that `parent` starts.
async fn start_child(sandbox: &Sandbox, client: &Client, parent: &SessionId) -> SessionId {
    client
        .request::<SessionNew>(&SessionNewParams {
            cwd: sandbox.workspace.clone(),
            environment: sandbox.client_environment(),
            agent: None,
            setup: SessionSetup::default(),
            parent: Some(parent.clone()),
        })
        .await
        .unwrap()
        .session
}

/// The job of `session`'s turns, as `rho/job/list` reports it now.
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

// Covers: the sessions a session starts are child jobs of its job, and
// their turns queue on `behavior.agent_concurrency` in the order they were
// prompted, each prompt answering once its turn queued: one runs, the next
// waits first in line, a queued turn cancelled ends `cancelled` without
// running, and the waiting turn runs once the running one frees its slot.
// Owner: daemon session jobs and the shared agent budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_a_session_starts_are_its_children_and_share_the_agent_budget() {
    let sandbox = Sandbox::new();
    sandbox.set_config("bypass", "agent_concurrency = 1\n");
    let client = sandbox.start_daemon("tool-call").await;
    let parent = sandbox.new_session(&client).await;
    let [running, waiting, cancelled] = [
        start_child(&sandbox, &client, &parent).await,
        start_child(&sandbox, &client, &parent).await,
        start_child(&sandbox, &client, &parent).await,
    ];
    for child in [&running, &waiting, &cancelled] {
        sandbox.prompt_tool_call(&client, child, &blocked()).await;
    }
    let queued = sandbox
        .poll_until("the first turn to run", async || {
            let jobs = [
                job_of(&client, &running).await,
                job_of(&client, &waiting).await,
                job_of(&client, &cancelled).await,
            ];
            (jobs[0].state == JobState::Running).then_some(jobs)
        })
        .await;
    let parent_job = job_of(&client, &parent).await.id;
    let place = |position| {
        Some(QueuePlace {
            budget: Budget::AgentConcurrency,
            limit: 1,
            position,
        })
    };
    assert_eq!(
        queued.map(|job| (job.parent, job.state, job.queue)),
        [
            (Some(parent_job.clone()), JobState::Running, None),
            (Some(parent_job.clone()), JobState::Queued, place(1)),
            (Some(parent_job.clone()), JobState::Queued, place(2)),
        ],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );

    client
        .request::<SessionCancel>(&SessionCancelParams {
            session: cancelled.clone(),
        })
        .await
        .unwrap();
    let withdrawn = settled_job(&sandbox, &client, &cancelled).await;
    let still_waiting = job_of(&client, &waiting).await;
    std::fs::write(sandbox.workspace.join("go"), "").unwrap();
    let ended = [
        settled_job(&sandbox, &client, &running).await,
        settled_job(&sandbox, &client, &waiting).await,
    ];
    assert_eq!(
        (
            (withdrawn.state, withdrawn.queue),
            (still_waiting.state, still_waiting.queue),
            ended.map(|job| (job.state, job.queue)),
        ),
        (
            (JobState::Cancelled, None),
            (JobState::Queued, place(1)),
            [(JobState::Completed, None), (JobState::Completed, None)],
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a started session's turn lends its slot while subagents it started
// are under way, so with a one-slot budget its subagent runs and the turn,
// which waits for that subagent's result, completes instead of holding the
// only slot its child needs (#1714).
// Owner: daemon shared agent budget, started-session turns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_started_session_lends_its_slot_to_the_subagents_it_waits_on() {
    let sandbox = Sandbox::new();
    sandbox.set_config("bypass", "agent_concurrency = 1\n");
    let client = sandbox.start_daemon("tool-call").await;
    let parent = sandbox.new_session(&client).await;
    let child = start_child(&sandbox, &client, &parent).await;
    let delegate = serde_json::json!({
        "id": "delegate",
        "name": "agent",
        "arguments": {"agent_id": "explorer", "prompt": "Look around."},
    });

    sandbox.prompt_tool_call(&client, &child, &delegate).await;
    let ended = settled_job(&sandbox, &client, &child).await;
    let subagent = client
        .request::<JobList>(&JobListParams {})
        .await
        .unwrap()
        .jobs
        .into_iter()
        .find(|job| job.parent.as_ref() == Some(&ended.id))
        .map(|job| job.state);

    assert_eq!(
        (ended.state, subagent),
        (JobState::Completed, Some(JobState::Completed)),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a started session's turn waiting for a person lends its slot, so
// with a one-slot budget another started session runs meanwhile, while the
// first still waits for its answer (#1714).
// Owner: daemon shared agent budget, started-session turns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_started_session_waiting_for_a_person_lends_its_slot() {
    let sandbox = Sandbox::new();
    sandbox.set_config("supervised", "agent_concurrency = 1\n");
    let client = sandbox.start_daemon("tool-call").await;
    let parent = sandbox.new_session(&client).await;
    let [asking, other] = [
        start_child(&sandbox, &client, &parent).await,
        start_child(&sandbox, &client, &parent).await,
    ];
    let write = serde_json::json!({
        "id": "edit",
        "name": "write",
        "arguments": {"path": "note.txt", "content": "approved"},
    });

    sandbox.prompt_tool_call(&client, &asking, &write).await;
    sandbox
        .poll_until("the write to ask for approval", async || {
            (job_of(&client, &asking).await.state == JobState::WaitingForInput).then_some(())
        })
        .await;
    sandbox.prompt(&client, &other).await.unwrap();
    let ran = settled_job(&sandbox, &client, &other).await;

    assert_eq!(
        (ran.state, job_of(&client, &asking).await.state),
        (JobState::Completed, JobState::WaitingForInput),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
