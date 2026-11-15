use std::{path::Path, sync::Arc};

use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, SessionSetup, TurnId};
use tempfile::TempDir;
use tokio::sync::watch;

use super::super::{
    background::SessionBackground, idle_wake::try_give_up, test_sessions::load_waiting_on, Entered,
    LoadedSession, OpenWith, Sessions,
};
use crate::{
    app::{
        agent_executor::AgentRunHandle,
        daemon_host::jobs::{JobRegistry, JobStore, TurnEnd},
        session_runtime::BackgroundWait,
        subagent_manager::SubagentManager,
        subagent_pool::SubagentPool,
    },
    subagent::{RunState, RunStatus},
    tools::process::{KeepWarm, ProcessLimits, ProcessManager, ProcessRole},
};

/// A command that outlives every wait here, in the shell of either platform.
const LONG_RUNNING_COMMAND: &str = "sleep 300";

const SUBAGENT: &str = "abc123";

/// A session's job state or activity, as its clients hear them, in order.
#[derive(Debug, PartialEq)]
enum Heard {
    State(wire::JobState),
    Activity(wire::SessionActivity),
}

/// What the session's feed told of its states and activity.
fn heard(session: &LoadedSession) -> Vec<Heard> {
    session
        .feed
        .buffered_for_tests()
        .into_iter()
        .filter_map(|event| match event {
            wire::SessionEvent::State { state, .. } => Some(Heard::State(state)),
            wire::SessionEvent::Activity { state } => Some(Heard::Activity(state)),
            _ => None,
        })
        .collect()
}

/// The activity the session's clients heard last.
fn activity(session: &LoadedSession) -> Option<wire::SessionActivity> {
    heard(session)
        .into_iter()
        .rev()
        .find_map(|heard| match heard {
            Heard::Activity(state) => Some(state),
            Heard::State(_) => None,
        })
}

/// A session loaded in its own daemon whose work waits as `wait`, from
/// these managers.
struct Fixture {
    _root: TempDir,
    _home: TempDir,
    sessions: Arc<Sessions>,
    session: Arc<LoadedSession>,
    processes: ProcessManager,
    subagents: SubagentManager,
}

impl Fixture {
    async fn new(wait: BackgroundWait) -> Self {
        let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let sessions = Sessions::new(
            Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
            home.path(),
        );
        let processes = ProcessManager::new(ProcessLimits::default());
        let subagents = SubagentManager::new(
            crate::config::Config::default(),
            home.path().join("config.toml"),
            home.path().to_path_buf(),
            SubagentPool::Own,
        );
        let session = load_waiting_on(&sessions, &root, |id| {
            SessionBackground::for_tests(id, wait, Some(processes.clone()), Some(subagents.clone()))
        })
        .await;
        // As the session is added.
        session.report_activity();
        Self {
            _root: root,
            _home: home,
            sessions,
            session,
            processes,
            subagents,
        }
    }

    /// Starts a turn, as a prompt or the daemon's delivery does, without
    /// running it.
    fn start_turn(&self) -> TurnId {
        let session = &self.session;
        let with = OpenWith {
            environment: session.environment.current(),
            setup: SessionSetup::default(),
            grant: self.sessions.jobs.grant_of(&session.job).unwrap(),
        };
        let mut slot = session.turn();
        match self.sessions.start_turn(
            session, &mut slot, &with, /*key*/ None, /*limit*/ None,
        ) {
            Ok(Entered::Started(turn)) => turn.id,
            _ => panic!("the idle session starts a turn"),
        }
    }

    /// Ends `turn` as `end`, as the turn's task does.
    fn end_turn(&self, turn: TurnId, end: TurnEnd) {
        let outcome = match end {
            TurnEnd::Completed => wire::TurnOutcome::Answered {
                text: "done".into(),
                stop: wire::AnswerStop::EndTurn,
            },
            _ => wire::TurnOutcome::Cancelled,
        };
        let _slot = self.session.turn();
        self.session.end_turn(turn, Some(outcome), end);
    }

    async fn start_process(&self, role: ProcessRole) {
        self.processes
            .start_as(LONG_RUNNING_COMMAND.into(), Path::new("."), None, role)
            .await
            .unwrap();
    }

    /// Runs a subagent of the session until its returned senders say it
    /// finished.
    fn run_subagent(&self) -> (watch::Sender<RunStatus>, watch::Sender<bool>) {
        let (status, status_rx) = watch::channel(RunStatus {
            state: RunState::Running,
            ..Default::default()
        });
        let (completion, completion_rx) = watch::channel(false);
        self.subagents.insert_handle_for_test(
            SUBAGENT,
            self.session.id.as_str(),
            AgentRunHandle::controlled_for_test(
                status_rx,
                completion_rx,
                rho_tools::cancellation::RunCancellation::new(),
            ),
        );
        (status, completion)
    }

    fn finish_subagent(&self) {
        self.subagents.insert_completed_status_for_test(
            SUBAGENT,
            self.session.id.as_str(),
            RunStatus {
                state: RunState::Ok,
                ..Default::default()
            },
        );
    }

    async fn shutdown(self) {
        self.processes.shutdown().await;
    }
}

const TASK: ProcessRole = ProcessRole::Task {
    keep_warm: KeepWarm::Off,
};

/// What a session started before its turn was cancelled.
#[derive(Clone, Copy, Debug)]
enum Left {
    Task,
    Service,
    Subagent,
}

// Covers: a session settles once its turn ends unless the daemon or its host
// will start a turn for work it waits on: a session whose prompts hold for
// their work is never woken, so a cancelled turn that left a task running
// still settles; a service never keeps a session from settling; a task or
// subagent the daemon or host delivers keeps it active. A wrong level hangs
// `wait` or returns it before the result. Owner: daemon session settle.
#[tokio::test]
async fn a_session_settles_unless_its_waker_will_start_a_turn() {
    use wire::SessionActivity::{Active, Settled};
    let cases = [
        (BackgroundWait::HoldPrompt, Left::Task, Settled),
        (BackgroundWait::HoldPrompt, Left::Subagent, Settled),
        (BackgroundWait::HostTurns, Left::Service, Settled),
        (BackgroundWait::HostTurns, Left::Task, Active),
        (BackgroundWait::HostTurns, Left::Subagent, Active),
        (BackgroundWait::HostWakes, Left::Service, Settled),
        (BackgroundWait::HostWakes, Left::Task, Active),
    ];
    for (wait, left, expected) in cases {
        let fixture = Fixture::new(wait).await;
        let turn = fixture.start_turn();
        let _subagent = match left {
            Left::Task => {
                fixture.start_process(TASK).await;
                None
            }
            Left::Service => {
                fixture.start_process(ProcessRole::Service).await;
                None
            }
            Left::Subagent => Some(fixture.run_subagent()),
        };
        fixture.end_turn(turn, TurnEnd::Cancelled);

        assert_eq!(
            activity(&fixture.session),
            Some(expected),
            "{wait:?} with a {left:?} left running"
        );
        fixture.shutdown().await;
    }
}

// Covers: a subagent that finishes while its session idles keeps the session
// active until the daemon's delivery turn took its result and ended, and
// `active` precedes the turn's `running`, `settled` follows its end. A
// `settled` between the finish and the delivery turn returns `wait` before
// the result. Owner: daemon session settle.
#[tokio::test]
async fn a_finished_subagent_keeps_its_session_active_until_its_delivery_ends() {
    use wire::{
        JobState::{Cancelled, Completed, Running},
        SessionActivity::{Active, Settled},
    };
    let fixture = Fixture::new(BackgroundWait::HostTurns).await;
    let prompt = fixture.start_turn();
    let (status, completion) = fixture.run_subagent();
    fixture.end_turn(prompt, TurnEnd::Cancelled);
    status.send_modify(|status| status.state = RunState::Ok);
    completion.send(true).unwrap();
    // As the background watcher does once the subagent ended.
    fixture.session.report_activity();
    let delivery = fixture.start_turn();
    fixture.subagents.observe(SUBAGENT);
    fixture.end_turn(delivery, TurnEnd::Completed);

    assert_eq!(
        heard(&fixture.session),
        vec![
            Heard::Activity(Settled),
            Heard::Activity(Active),
            Heard::State(Running),
            Heard::State(Cancelled),
            Heard::State(Running),
            Heard::State(Completed),
            Heard::Activity(Settled),
        ]
    );
    fixture.shutdown().await;
}

// Covers: a delivery turn that ended without taking its result, with
// nothing left running, is not started again, so the watcher gives up and
// the session settles instead of staying active forever; a give-up while
// the delivery turn still runs, or for a turn that is no longer the last,
// changes nothing, and the next turn makes the session active again.
// Owner: daemon session settle and idle wake.
#[tokio::test]
async fn a_delivery_that_left_its_result_settles_once_the_watcher_gives_up() {
    use wire::SessionActivity::{Active, Settled};
    let fixture = Fixture::new(BackgroundWait::HostTurns).await;
    fixture.finish_subagent();
    fixture.session.report_activity();
    let work = fixture.session.background.awaited().unwrap();
    let delivery = fixture.start_turn();
    // (whether it gave up, the activity heard after)
    let mut seen = vec![(
        try_give_up(&fixture.session, &work, &delivery),
        activity(&fixture.session),
    )];
    fixture.end_turn(delivery.clone(), TurnEnd::Cancelled);
    for turn in [TurnId::new("an earlier delivery"), delivery] {
        let gave_up = try_give_up(&fixture.session, &work, &turn);
        seen.push((gave_up, activity(&fixture.session)));
    }
    let next = fixture.start_turn();
    let rearmed = activity(&fixture.session);
    fixture.end_turn(next, TurnEnd::Cancelled);

    assert_eq!(
        (seen, rearmed),
        (
            vec![
                (false, Some(Active)),
                (false, Some(Active)),
                (true, Some(Settled))
            ],
            Some(Active)
        )
    );
    fixture.shutdown().await;
}
