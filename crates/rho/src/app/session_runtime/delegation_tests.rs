use std::{path::Path, time::Duration};

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
    BoundaryInputRequest, InputBoundary, Rho, RunEvent, SessionOptions, UserInput,
};

use super::{BackgroundSources, BackgroundWait, Delegation};
use crate::{
    app::{
        agent_executor::AgentRunHandle,
        pull_requests::{PullRequestInbox, PullRequests, ScriptedForge},
        subagent_manager::SubagentManager,
    },
    subagent::{RunState, RunStatus},
    tools::{
        process::{KeepWarm, ProcessLimits, ProcessManager, ProcessRole},
        workflow_tracker::{WorkflowFinishedSnapshot, WorkflowRunTracker},
    },
};

#[cfg(unix)]
const LONG_RUNNING_COMMAND: &str = "sleep 300";
#[cfg(windows)]
const LONG_RUNNING_COMMAND: &str = "Start-Sleep -Seconds 300";

/// A plain `process start`: a task with no keep-warm hold.
const TASK: ProcessRole = ProcessRole::Task {
    keep_warm: KeepWarm::Off,
};

/// Every fixture session's id, so work from an "earlier prompt" can be
/// registered to it before the fixture's prompt starts.
const SESSION_ID: &str = "headless-fixture";

/// The pull request the fixture session watches.
const PULL_REQUEST: &str = "acme/widgets#12";

/// When the fixture session watches [`PULL_REQUEST`], in unix milliseconds.
const WATCHED_AT: u64 = 1_700_000_000_000;

/// A PR service over a scripted forge in `dir`.
struct PullRequestFixture {
    service: PullRequests,
    forge: std::sync::Arc<ScriptedForge>,
    /// The fixture session's inbox, which watches [`PULL_REQUEST`], its
    /// check `ci` running.
    inbox: PullRequestInbox,
}

impl PullRequestFixture {
    async fn watching(dir: &Path) -> Self {
        let forge = ScriptedForge::new();
        let service = PullRequests::open(dir, forge.clone());
        let inbox = service.inbox(&session_id(), dir);
        forge.set_open(PULL_REQUEST, &["ci"], &[]);
        inbox.watch(PULL_REQUEST, WATCHED_AT).await.unwrap();
        Self {
            service,
            forge,
            inbox,
        }
    }

    /// Check `ci` failed, as the next poll tells the session.
    async fn fail_check(&self) {
        self.forge.set_open(PULL_REQUEST, &[], &["ci"]);
        self.service.tick(WATCHED_AT + 60_000).await;
        assert!(self.inbox.has_news(), "the failed check is news");
    }
}

fn session_id() -> rho_sdk::SessionId {
    rho_sdk::SessionId::from_string(SESSION_ID).unwrap()
}

/// Whether the fixture registers a running child before the prompt starts.
enum Child {
    Running,
    Absent,
}

struct Fixture {
    _root: tempfile::TempDir,
    session: rho_sdk::Session,
    manager: SubagentManager,
    processes: ProcessManager,
    workflows: WorkflowRunTracker,
    host: Delegation,
    run: rho_sdk::Run,
    status: tokio::sync::watch::Sender<RunStatus>,
    completion: tokio::sync::watch::Sender<bool>,
    child_cancel: rho_tools::cancellation::RunCancellation,
}

impl Fixture {
    async fn new() -> Self {
        Self::build(
            Child::Running,
            BackgroundWait::HoldPrompt,
            ProcessManager::new(ProcessLimits::default()),
            WorkflowRunTracker::new(),
            /*pull_requests*/ None,
        )
        .await
    }

    /// `processes` and `workflows` may already hold work from an earlier
    /// prompt; running work holds the prompt as `wait` says.
    async fn build(
        child: Child,
        wait: BackgroundWait,
        processes: ProcessManager,
        workflows: WorkflowRunTracker,
        pull_requests: Option<PullRequestInbox>,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let manager = SubagentManager::new(
            crate::config::Config::default(),
            root.path().join("config.toml"),
            root.path().to_path_buf(),
            crate::app::subagent_pool::SubagentPool::Own,
        );
        let runtime = Rho::builder()
            .provider(ScriptedProvider::new(
                ModelIdentity::new("scripted", "test", "model"),
                ["initial answer", "result incorporated"].map(|text| {
                    ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                        text.into(),
                    )]))
                }),
            ))
            .build()
            .unwrap();
        let session = runtime
            .session(SessionOptions::new().id(rho_sdk::SessionId::from_string(SESSION_ID).unwrap()))
            .await
            .unwrap();
        let host = Delegation::attach(
            &session,
            BackgroundSources {
                surfaces: None,
                pull_requests: pull_requests.as_ref(),
                subagents: Some(&manager),
                processes: Some(&processes),
                workflows: &workflows,
                peer_sessions: None,
            },
            wait,
            None,
            crate::session::SessionPersistence::Saved,
            /*held*/ &mut None,
        )
        .unwrap();
        let (status, status_rx) = tokio::sync::watch::channel(RunStatus {
            state: RunState::Running,
            ..Default::default()
        });
        let (completion, completion_rx) = tokio::sync::watch::channel(false);
        let child_cancel = rho_tools::cancellation::RunCancellation::new();
        match child {
            Child::Running => manager.insert_handle_for_test(
                "abc123",
                session.id().as_str(),
                AgentRunHandle::controlled_for_test(status_rx, completion_rx, child_cancel.clone()),
            ),
            Child::Absent => {}
        }
        let run = session
            .start(UserInput::text("delegate work"))
            .await
            .unwrap();
        Self {
            _root: root,
            session,
            manager,
            processes,
            workflows,
            host,
            run,
            status,
            completion,
            child_cancel,
        }
    }

    async fn final_boundary(&mut self) -> BoundaryInputRequest {
        let first = self.host.requests.recv().await.unwrap();
        assert_eq!(first.boundary(), InputBoundary::BeforeProvider);
        self.host.respond(first).await;
        let request = loop {
            tokio::select! {
                request = self.host.requests.recv() => break request.unwrap(),
                event = self.run.next_event() => assert!(!matches!(event, None | Some(RunEvent::Completed { .. }))),
            }
        };
        assert_eq!(request.boundary(), InputBoundary::BeforeCompletion);
        request
    }

    /// Drives the run to its end, returning the final text and the boundary
    /// inputs it applied.
    async fn finish_run(self) -> (String, Vec<UserInput>) {
        let Self {
            mut host, mut run, ..
        } = self;
        let applied = Delegation::drive(&mut host, async {
            let mut applied = Vec::new();
            while let Some(event) = run.next_event().await {
                if let RunEvent::BoundaryInputApplied { input, .. } = event {
                    applied.push(input);
                }
            }
            applied
        })
        .await;
        (run.outcome().await.unwrap().text().to_owned(), applied)
    }

    async fn start_process(&self, role: ProcessRole) -> String {
        self.processes
            .start_as(LONG_RUNNING_COMMAND.into(), Path::new("."), None, role)
            .await
            .unwrap()
            .process_id
    }

    /// Stops a process and waits until its exit is recorded.
    async fn stop_process(processes: &ProcessManager, id: &str) {
        let exited = processes.notified_owned();
        processes.stop(id, Duration::ZERO).await.unwrap();
        exited.await;
    }

    fn finish_workflow(workflows: &WorkflowRunTracker, run_id: &str) {
        workflows.mark_finished(
            run_id,
            WorkflowFinishedSnapshot {
                lifecycle: "completed".into(),
                outcome: Some("success".into()),
                nodes: Vec::new(),
                error: None,
                outputs: Vec::new(),
            },
        );
    }
}

/// Background work a final-boundary case finishes after the first poll.
#[derive(Clone, Copy)]
enum Work {
    Child,
    ProcessDuringPrompt(ProcessRole),
    Workflow,
    ProcessFromEarlierPrompt,
    WorkflowFromEarlierPrompt,
}

// Covers: a child, a workflow, or a process task started during the prompt
// holds natural completion, and its result continues the original run exactly
// once with display rows the host can claim; a process service started during
// the prompt does not hold it, nor does a process or workflow left from an
// earlier prompt, such as a `/workflow:<name>` start, nor one under a host that
// wakes the idle session (T3 Code).
// The SDK channel tests do not own this wait.
// Owner: shared CLI/daemon background delivery lifecycle.
#[tokio::test]
async fn background_work_holds_or_releases_natural_completion() {
    use BackgroundWait::{HoldPrompt, HostWakes};
    let held = ("result incorporated", 1, (0, 0, 0));
    let cases = [
        ("child", Work::Child, HoldPrompt, held),
        (
            "process task started during the prompt",
            Work::ProcessDuringPrompt(TASK),
            HoldPrompt,
            held,
        ),
        (
            "process service started during the prompt",
            Work::ProcessDuringPrompt(ProcessRole::Service),
            HoldPrompt,
            ("initial answer", 0, (0, 0, 1)),
        ),
        ("workflow", Work::Workflow, HoldPrompt, held),
        (
            "process from an earlier prompt",
            Work::ProcessFromEarlierPrompt,
            HoldPrompt,
            ("initial answer", 0, (0, 0, 1)),
        ),
        (
            "workflow from an earlier prompt",
            Work::WorkflowFromEarlierPrompt,
            HoldPrompt,
            ("initial answer", 0, (0, 1, 0)),
        ),
        (
            "process started during the prompt when the host wakes the idle session",
            Work::ProcessDuringPrompt(TASK),
            HostWakes,
            ("initial answer", 0, (0, 0, 1)),
        ),
    ];
    for (label, work, wait, (text, applied, undelivered)) in cases {
        let processes = ProcessManager::new(ProcessLimits::default());
        let workflows = WorkflowRunTracker::new();
        let earlier = match work {
            Work::ProcessFromEarlierPrompt => Some(
                processes
                    .start(LONG_RUNNING_COMMAND.into(), Path::new("."), None)
                    .await
                    .unwrap()
                    .process_id,
            ),
            Work::WorkflowFromEarlierPrompt => {
                // Owned by the session `build` creates, as an earlier prompt's
                // `/workflow:<name>` start would be.
                assert!(workflows.register_start(
                    "wf-run",
                    "fixture",
                    "digest",
                    Some(SESSION_ID.into())
                ));
                None
            }
            Work::Child | Work::ProcessDuringPrompt(_) | Work::Workflow => None,
        };
        let child = match work {
            Work::Child => Child::Running,
            Work::ProcessDuringPrompt(_)
            | Work::Workflow
            | Work::ProcessFromEarlierPrompt
            | Work::WorkflowFromEarlierPrompt => Child::Absent,
        };
        let mut fixture = Fixture::build(
            child, wait, processes, workflows, /*pull_requests*/ None,
        )
        .await;
        let mut displays = fixture.host.offered_displays();
        let request = fixture.final_boundary().await;
        let during = match work {
            Work::ProcessDuringPrompt(role) => Some(fixture.start_process(role).await),
            Work::Workflow => {
                assert!(fixture
                    .workflows
                    .register_start("wf-run", "fixture", "digest", None));
                None
            }
            Work::Child | Work::ProcessFromEarlierPrompt | Work::WorkflowFromEarlierPrompt => None,
        };
        {
            let reply = fixture.host.respond(request);
            tokio::pin!(reply);
            // The first poll decides whether to hold. Finishing the work only
            // afterwards shows whether the boundary waited for it.
            let _ = futures_util::poll!(&mut reply);
            match work {
                Work::Child => {
                    fixture.status.send_replace(RunStatus {
                        state: RunState::Ok,
                        result: Some("child finding".into()),
                        ..Default::default()
                    });
                    fixture.completion.send_replace(true);
                }
                Work::ProcessDuringPrompt(_) => {
                    Fixture::stop_process(&fixture.processes, during.as_deref().unwrap()).await;
                }
                Work::Workflow | Work::WorkflowFromEarlierPrompt => {
                    Fixture::finish_workflow(&fixture.workflows, "wf-run");
                }
                Work::ProcessFromEarlierPrompt => {
                    Fixture::stop_process(&fixture.processes, earlier.as_deref().unwrap()).await;
                }
            }
            reply.await;
        }
        let session_id = fixture.session.id().to_string();
        let (manager, processes, workflows) = (
            fixture.manager.clone(),
            fixture.processes.clone(),
            fixture.workflows.clone(),
        );
        let (final_text, applied_inputs) = fixture.finish_run().await;
        let shown = applied_inputs
            .iter()
            .map(|input| displays.applied(input).map(|display| display.0.len()))
            .collect::<Vec<_>>();
        assert_eq!(
            (
                final_text.as_str(),
                shown,
                (
                    manager.take_notifications(&session_id).len(),
                    workflows.take_notifications(&session_id).len(),
                    processes.take_notifications().len(),
                ),
            ),
            (text, vec![Some(1); applied], undelivered),
            "{label}"
        );
    }
}

// Covers: an offer the runtime rejected must never be shown, even when a
// later applied input is claimed after it.
// Owner: shared CLI/daemon delivery display matching.
#[test]
fn applied_input_claims_only_its_own_display() {
    use crate::display_transcript::{DisplayRow, DisplayTranscript};

    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut displays = super::OfferedDisplays { receiver };
    let display = |text: &str| DisplayTranscript(vec![DisplayRow::Notice(text.into())]);
    for offer in ["rejected", "applied"] {
        sender
            .send((UserInput::text(offer), display(offer)))
            .unwrap();
    }
    assert_eq!(
        (
            displays.applied(&UserInput::text("applied")),
            displays.applied(&UserInput::text("rejected")),
        ),
        (Some(display("applied")), None)
    );
}

// Covers: cancellation while natural completion is held does not wait for a
// child result or issue another provider request; teardown cancels the child.
// Owner: shared CLI/daemon delegation lifecycle.
#[tokio::test]
async fn cancellation_interrupts_child_wait_and_shutdown_stops_child() {
    let mut fixture = Fixture::new().await;
    let request = fixture.final_boundary().await;
    let waiting = fixture.host.respond(request);
    tokio::pin!(waiting);
    assert!(futures_util::poll!(&mut waiting).is_pending());
    fixture.run.cancel();
    while fixture.run.next_event().await.is_some() {}
    assert!(matches!(
        fixture.run.outcome().await,
        Err(rho_sdk::Error::Cancelled)
    ));
    let shutdown = fixture.manager.shutdown();
    tokio::pin!(shutdown);
    assert!(futures_util::poll!(&mut shutdown).is_pending());
    fixture.child_cancel.cancelled().await;
    fixture.status.send_replace(RunStatus {
        state: RunState::Stopped,
        ..Default::default()
    });
    fixture.completion.send_replace(true);
    shutdown.await;
    assert_eq!(
        fixture.manager.status("abc123").unwrap().status.state,
        RunState::Stopped
    );
}

// Covers: a cancelled SDK boundary must restore every result it did not
// accept, whichever source it came from.
// Owner: shared CLI/daemon delivery receipts.
#[tokio::test]
async fn rejected_boundary_restores_terminal_results() {
    let mut fixture = Fixture::new().await;
    let request = fixture.final_boundary().await;
    fixture.status.send_replace(RunStatus {
        state: RunState::Ok,
        result: Some("child finding".into()),
        ..Default::default()
    });
    fixture.completion.send_replace(true);
    let process_id = fixture.start_process(TASK).await;
    Fixture::stop_process(&fixture.processes, &process_id).await;
    assert!(fixture
        .workflows
        .register_start("wf-run", "fixture", "digest", None));
    Fixture::finish_workflow(&fixture.workflows, "wf-run");
    fixture.run.cancel();
    while fixture.run.next_event().await.is_some() {}
    assert!(matches!(
        fixture.run.outcome().await,
        Err(rho_sdk::Error::Cancelled)
    ));
    fixture.host.respond(request).await;
    let session_id = fixture.session.id().as_str();
    let restored = (
        fixture
            .manager
            .take_notifications(session_id)
            .into_iter()
            .map(|notification| notification.snapshot.id)
            .collect::<Vec<_>>(),
        fixture
            .processes
            .take_notifications()
            .into_iter()
            .map(|notification| notification.process_id)
            .collect::<Vec<_>>(),
        fixture
            .workflows
            .take_notifications(session_id)
            .into_iter()
            .map(|notification| notification.run_id)
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        restored,
        (
            vec!["abc123".to_owned()],
            vec![process_id],
            vec!["wf-run".to_owned()]
        )
    );
}

// Covers: ending the event pump immediately after input commits must still
// reap its receipt, or the next prompt would receive the same result again.
// Owner: shared CLI/daemon delivery receipts.
#[tokio::test]
async fn accepted_boundary_is_reaped_when_event_pump_returns() {
    let mut fixture = Fixture::new().await;
    fixture.status.send_replace(RunStatus {
        state: RunState::Ok,
        result: Some("child finding".into()),
        ..Default::default()
    });
    fixture.completion.send_replace(true);
    Delegation::drive(&mut fixture.host, async {
        loop {
            match fixture.run.next_event().await {
                Some(RunEvent::BoundaryInputApplied { .. }) => break,
                Some(_) => {}
                None => panic!("run ended before child delivery"),
            }
        }
    })
    .await;
    assert!(fixture
        .manager
        .take_notifications(fixture.session.id().as_str())
        .is_empty());
    fixture.run.cancel();
    while fixture.run.next_event().await.is_some() {}
    let _ = fixture.run.outcome().await;
}

// Covers: pull request news a boundary offered stays queued, on disk too,
// until the SDK applied that boundary: a rejected one puts it back for the
// next, and an applied one removes it for good, so a crash never loses news
// and a delivered one is never told again.
// Owner: shared CLI/daemon delivery receipts.
#[tokio::test]
async fn pull_request_news_is_acknowledged_only_when_its_boundary_is_applied() {
    let dir = tempfile::tempdir().unwrap();
    let pull_requests = PullRequestFixture::watching(dir.path()).await;
    let inbox = &pull_requests.inbox;
    // What a daemon restarted now would find queued.
    let saved = || PullRequests::open(dir.path(), ScriptedForge::new()).has_news(&session_id());
    let fixture = || {
        Fixture::build(
            Child::Absent,
            BackgroundWait::HoldPrompt,
            ProcessManager::new(ProcessLimits::default()),
            WorkflowRunTracker::new(),
            Some(inbox.clone()),
        )
    };

    // Rejected: the run is cancelled before its last boundary is answered.
    let mut rejecting = fixture().await;
    let request = rejecting.final_boundary().await;
    pull_requests.fail_check().await;
    rejecting.run.cancel();
    while rejecting.run.next_event().await.is_some() {}
    rejecting.host.respond(request).await;
    drop(rejecting);
    let rejected = (inbox.has_news(), saved());
    let news = inbox.take_news();
    inbox.restore_news(&news);

    // Applied: the next run's first boundary offers it.
    let mut fixture = fixture().await;
    let applied = Delegation::drive(&mut fixture.host, async {
        loop {
            match fixture.run.next_event().await {
                Some(RunEvent::BoundaryInputApplied { input, .. }) => break input,
                Some(_) => {}
                None => panic!("run ended before the news was delivered"),
            }
        }
    })
    .await;
    let acknowledged = (inbox.has_news(), saved());
    fixture.run.cancel();
    while fixture.run.next_event().await.is_some() {}
    let _ = fixture.run.outcome().await;

    assert_eq!(
        (rejected, applied, acknowledged),
        ((true, true), UserInput::text(news.prompt()), (false, false))
    );
}

// Covers: routine notices cannot wake an idle parent, while an action request
// releases the held boundary even though the requesting child is still alive.
// Owner: shared CLI/daemon notice scheduling.
#[tokio::test]
async fn action_request_wakes_parent_and_includes_earlier_notices() {
    use crate::app::subagent_messaging::{NoticeDelivery, SubagentNotice};

    let mut fixture = Fixture::new().await;
    let request = fixture.final_boundary().await;
    let ordinary = SubagentNotice {
        run_id: "abc123".into(),
        agent_id: "fixture".into(),
        parent_session_id: fixture.session.id().clone(),
        message: "finding".into(),
        delivery: NoticeDelivery::NextTurn,
        acknowledged: Default::default(),
    };
    fixture.manager.post_notice_for_test(ordinary.clone());
    let action = SubagentNotice {
        message: "need parent decision".into(),
        delivery: NoticeDelivery::ParentActionRequired,
        acknowledged: Default::default(),
        ..ordinary.clone()
    };
    {
        let reply = fixture.host.respond(request);
        tokio::pin!(reply);
        assert!(futures_util::poll!(&mut reply).is_pending());
        fixture.manager.post_notice_for_test(action.clone());
        reply.await;
    }
    assert!(ordinary.is_acknowledged());
    assert!(action.is_acknowledged());
    assert!(fixture
        .manager
        .has_running_for_session(fixture.session.id().as_str()));
    fixture.run.cancel();
    while fixture.run.next_event().await.is_some() {}
    assert!(matches!(
        fixture.run.outcome().await,
        Err(rho_sdk::Error::Cancelled)
    ));
}

// Covers: a failed attach must leave the held notice binding with the caller,
// or the session would drop child notices it promised to deliver later.
// Owner: shared CLI/daemon notice binding handoff.
#[tokio::test]
async fn failed_attach_keeps_held_notices() {
    let Fixture {
        session,
        manager,
        processes,
        workflows,
        host,
        mut run,
        ..
    } = Fixture::new().await;
    let mut held = host.hold_notices();
    assert!(held.is_some());
    // The fixture's run is still active, so installing boundary inputs fails.
    let attached = Delegation::attach(
        &session,
        BackgroundSources {
            surfaces: None,
            pull_requests: None,
            subagents: Some(&manager),
            processes: Some(&processes),
            workflows: &workflows,
            peer_sessions: None,
        },
        BackgroundWait::HoldPrompt,
        None,
        crate::session::SessionPersistence::Saved,
        &mut held,
    );
    assert!(attached.is_err());
    assert!(held.is_some());
    run.cancel();
    while run.next_event().await.is_some() {}
    let _ = run.outcome().await;
}

/// What a session's earlier prompts left behind.
#[derive(Clone, Copy)]
enum Left {
    /// A process, which exited first when `exits`.
    Process { role: ProcessRole, exits: bool },
    /// News on a pull request it watches.
    PullRequestNews,
}

// Covers: a live process of either kind, or a finished task's undelivered
// result, is background work that keeps an idle session loaded and a daemon
// upgrade waiting; a finished service's result is not, since it waits only
// for the next prompt and must not pin the session; nor is queued pull
// request news, which is saved, so an unloaded session or a restarted
// daemon still delivers it.
// Owner: session runtime background-work predicate (unload, upgrade, delete).
#[tokio::test]
async fn only_live_processes_and_task_results_are_background_work() {
    let process = |role, exits| Left::Process { role, exits };
    // (name, what is left, whether it is work)
    let cases = [
        ("live task", process(TASK, false), true),
        ("live service", process(ProcessRole::Service, false), true),
        ("finished task", process(TASK, true), true),
        (
            "finished service",
            process(ProcessRole::Service, true),
            false,
        ),
        ("pull request news", Left::PullRequestNews, false),
    ];
    for (name, left, expected) in cases {
        let processes = ProcessManager::new(ProcessLimits::default());
        let workflows = WorkflowRunTracker::new();
        let dir = tempfile::tempdir().unwrap();
        let mut pull_requests = None;
        match left {
            Left::Process { role, exits } => {
                let id = processes
                    .start_as(LONG_RUNNING_COMMAND.into(), Path::new("."), None, role)
                    .await
                    .unwrap()
                    .process_id;
                if exits {
                    Fixture::stop_process(&processes, &id).await;
                }
            }
            Left::PullRequestNews => {
                let watched = PullRequestFixture::watching(dir.path()).await;
                watched.fail_check().await;
                pull_requests = Some(watched);
            }
        }
        let sources = BackgroundSources {
            surfaces: None,
            pull_requests: pull_requests.as_ref().map(|watched| &watched.inbox),
            subagents: None,
            processes: Some(&processes),
            workflows: &workflows,
            peer_sessions: None,
        };
        assert_eq!(sources.has_work(SESSION_ID), expected, "{name}");
    }
}
