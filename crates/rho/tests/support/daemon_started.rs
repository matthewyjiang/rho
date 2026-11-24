//! Sessions another session starts (#1714): the daemon records the job
//! writing the starting session as the new session's parent, and the new
//! session's turns wait for capacity in the daemon-wide agent budget, as
//! subagent runs do. A new session opens in the permission mode and on the
//! model a person chose for the sessions its parent starts, and keeps that
//! choice for the sessions it starts.

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{JobList, SessionCancel, SessionConfigure, SessionNew},
    Budget, Client, JobListParams, JobState, JobSummary, PermissionMode, QueuePlace,
    SessionCancelParams, SessionChange, SessionConfigureParams, SessionEvent, SessionId,
    SessionNewParams, SessionNewResponse, SessionOptions, SessionSetup, StartedSessionDefaults,
};

use super::sessions::{settled_job, Follower, Sandbox};

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
    open_child(sandbox, client, parent).await.session
}

/// Opens a session in the workspace that `parent` starts; the daemon's
/// answer.
async fn open_child(sandbox: &Sandbox, client: &Client, parent: &SessionId) -> SessionNewResponse {
    client
        .request::<SessionNew>(&SessionNewParams {
            cwd: sandbox.workspace.clone(),
            environment: sandbox.client_environment(),
            agent: None,
            setup: SessionSetup::default(),
            parent: Some(parent.clone()),
            grant: None,
            worktree: false,
        })
        .await
        .unwrap()
}

/// Applies `change` to `session`; its options then.
async fn configure(client: &Client, session: &SessionId, change: SessionChange) -> SessionOptions {
    client
        .request::<SessionConfigure>(&SessionConfigureParams {
            session: session.clone(),
            change,
        })
        .await
        .unwrap()
        .options
}

/// Attaches to `session`: its options, and the event its reset replay
/// sends right after the agent it runs as.
async fn attached(
    sandbox: &Sandbox,
    session: &SessionId,
) -> (SessionOptions, Option<SessionEvent>) {
    let (mut follower, attached) = Follower::attach(sandbox, session, Vec::new(), None).await;
    follower
        .read_until(|event| !matches!(event, SessionEvent::Agent { .. }))
        .await;
    let after_agent = follower.events.pop().map(|(_, event)| event);
    (attached.options, after_agent)
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

// Covers: the permission mode and model a person chose for the sessions a
// session starts outlast a daemon restart, and a reset replay of that
// session reports them; a session it starts then opens in that mode on
// that model, and keeps them as its own choice, so a session that one
// starts does too. The choosing session itself, and a session it started
// before it chose, keep no settings of their own: after the restart both
// follow the config's new mode.
// Owner: daemon sessions other sessions start.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_started_session_opens_with_what_a_person_chose_for_its_parent() {
    let sandbox = Sandbox::new();
    // Auto needs a classifier model, as a person's config gives it.
    let classifier = r#"[internal_agents.permission-classifier]
provider = "xai"
model = "grok-fixture"
auth = "xai-oauth"
"#;
    sandbox.set_config("bypass", classifier);
    let (mut daemon, client) = sandbox.spawn_daemon(sandbox.rho("fixed"), &[]).await;
    let parent = sandbox.new_session(&client).await;
    let unchosen = start_child(&sandbox, &client, &parent).await;
    // Both saved, so the next daemon has their files to reopen.
    for session in [&parent, &unchosen] {
        sandbox.prompt(&client, session).await.unwrap();
        settled_job(&sandbox, &client, session).await;
    }
    // The one model the fixture surely runs: the config's.
    let configured = open_child(&sandbox, &client, &parent)
        .await
        .options
        .model
        .current;
    configure(
        &client,
        &parent,
        SessionChange::StartedSessionPermissionMode {
            mode: Some(PermissionMode::Auto),
        },
    )
    .await;
    configure(
        &client,
        &parent,
        SessionChange::StartedSessionModel {
            model: Some(configured.clone()),
        },
    )
    .await;
    drop(client);
    let stop = sandbox
        .rho("fixed")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    assert!(stop.success(), "log:\n{}", sandbox.isolated.daemon_log());
    daemon.wait().unwrap();

    sandbox.set_config("supervised", classifier);
    let client = sandbox.start_daemon("fixed").await;
    // The attach loads the parent again, as its turn that starts a session
    // keeps it loaded.
    let (parent_options, reported) = attached(&sandbox, &parent).await;
    let (unchosen_options, _) = attached(&sandbox, &unchosen).await;
    let started = open_child(&sandbox, &client, &parent).await;
    let started_by_started = open_child(&sandbox, &client, &started.session).await;

    let settings = |options: &SessionOptions| {
        (
            options.permission_mode.current,
            options.model.current.clone(),
        )
    };
    let chosen = (PermissionMode::Auto, configured.clone());
    assert_eq!(
        (
            parent_options.permission_mode.current,
            unchosen_options.permission_mode.current,
            reported,
            settings(&started.options),
            settings(&started_by_started.options),
        ),
        (
            PermissionMode::Supervised,
            PermissionMode::Supervised,
            Some(SessionEvent::StartedSessionDefaults(
                StartedSessionDefaults {
                    permission_mode: Some(PermissionMode::Auto),
                    model: Some(configured),
                }
            )),
            chosen.clone(),
            chosen,
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a choice for the sessions a session starts that a person clears
// after the session reopened stays cleared once it reopens again, though
// the reopened session's conversation carried the saved choice: a reset
// replay reports none, and a session it starts opens in the config's mode.
// Owner: daemon sessions other sessions start.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cleared_choice_stays_cleared_once_the_session_reopens() {
    let sandbox = Sandbox::new();
    sandbox.set_config("bypass", "");
    let client = sandbox
        .start_daemon_with("fixed", &["--unload-idle-after", "200ms"])
        .await;
    let parent = sandbox.new_session(&client).await;
    let mode = |mode| SessionChange::StartedSessionPermissionMode { mode };
    // Kept attached, so the session stays loaded until the change applied.
    let (follower, _) = Follower::attach(&sandbox, &parent, Vec::new(), None).await;
    // Saved, so the unload has a file to reopen.
    sandbox.prompt(&client, &parent).await.unwrap();
    settled_job(&sandbox, &client, &parent).await;
    configure(&client, &parent, mode(Some(PermissionMode::Plan))).await;
    drop(follower);
    sandbox.wait_unloaded(&parent, /*times*/ 1).await;
    let (follower, _) = Follower::attach(&sandbox, &parent, Vec::new(), None).await;
    configure(&client, &parent, mode(None)).await;
    drop(follower);
    sandbox.wait_unloaded(&parent, /*times*/ 2).await;

    let (_, reported) = attached(&sandbox, &parent).await;
    let started = open_child(&sandbox, &client, &parent).await;

    assert_eq!(
        (reported, started.options.permission_mode.current),
        (
            Some(SessionEvent::StartedSessionDefaults(
                StartedSessionDefaults::default()
            )),
            PermissionMode::Bypass,
        ),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a session a subagent starts after the daemon stopped tracking the
// subagent's run, here one that ended, opens with the choice of the session
// that owns the subagent, found through the subagent's file, rather than on
// the config's, which may be looser.
// Owner: daemon sessions other sessions start.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_an_ended_subagent_starts_takes_its_owners_choice() {
    let sandbox = Sandbox::new();
    sandbox.set_config("bypass", "");
    // The fixture makes the call the prompt spells out as JSON.
    let client = sandbox.start_daemon("tool-call").await;
    let lead = sandbox.new_session(&client).await;
    configure(
        &client,
        &lead,
        SessionChange::StartedSessionPermissionMode {
            mode: Some(PermissionMode::Plan),
        },
    )
    .await;
    let call = serde_json::json!({
        "id": "delegate",
        "name": "agent",
        "arguments": {"agent_id": "explorer", "prompt": "Look around."},
    });
    sandbox.prompt_tool_call(&client, &lead, &call).await;
    let lead_job = settled_job(&sandbox, &client, &lead).await.id;
    let subagent = sandbox
        .poll_until("the subagent's run to end", async || {
            client
                .request::<JobList>(&JobListParams {})
                .await
                .unwrap()
                .jobs
                .into_iter()
                .find(|job| {
                    job.parent.as_ref() == Some(&lead_job) && job.state == JobState::Completed
                })
                .and_then(|job| job.session)
        })
        .await;

    let started = open_child(&sandbox, &client, &subagent).await;

    assert_eq!(
        started.options.permission_mode.current,
        PermissionMode::Plan,
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}
