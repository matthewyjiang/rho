use std::{path::Path, sync::Arc};

use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, SessionSetup, TurnId};
use tempfile::TempDir;
use tokio::{sync::watch, time::timeout};

use super::super::{
    background::SessionBackground,
    idle_wake::try_give_up,
    pull_request_wake::watch_for_tests,
    test_sessions::{load, load_waiting_on, seen, until, Load, Seen, TEST_BUDGET},
    Entered, LoadedSession, OpenWith, Sessions,
};
use crate::{
    app::{
        agent_executor::AgentRunHandle,
        daemon_host::jobs::{JobRegistry, JobStore, TurnEnd},
        pull_requests::ScriptedForge,
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

/// Where a fixture session's background work is read from.
#[derive(Clone, Copy)]
enum Work {
    /// The fixture's managers, waited for as `wait`.
    Managers(BackgroundWait),
    /// The session's runtime's tools, as the daemon reads them. Its client
    /// asked the daemon to wake it idle (`idle_wake: turn`), so they hold
    /// its pull request inbox.
    Tools,
}

/// A session loaded in its own daemon, whose pull requests `forge`
/// scripts.
struct Fixture {
    root: TempDir,
    _home: TempDir,
    forge: Arc<ScriptedForge>,
    sessions: Arc<Sessions>,
    session: Arc<LoadedSession>,
    processes: ProcessManager,
    subagents: SubagentManager,
}

impl Fixture {
    /// A session whose work waits as `wait`, from the fixture's managers.
    async fn new(wait: BackgroundWait) -> Self {
        Self::build(Work::Managers(wait)).await
    }

    /// A session whose work is its runtime's tools', its pull requests
    /// among them.
    async fn watching() -> Self {
        Self::build(Work::Tools).await
    }

    async fn build(work: Work) -> Self {
        let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let forge = ScriptedForge::new();
        let sessions = Sessions::new(
            Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
            home.path(),
            forge.clone(),
        );
        let processes = ProcessManager::new(ProcessLimits::default());
        let subagents = SubagentManager::new(
            crate::config::Config::default(),
            home.path().join("config.toml"),
            home.path().to_path_buf(),
            SubagentPool::Own,
        );
        let session = match work {
            Work::Managers(wait) => {
                load_waiting_on(&sessions, &root, |id| {
                    let (processes, subagents) = (Some(processes.clone()), Some(subagents.clone()));
                    SessionBackground::for_tests(id, wait, processes, subagents)
                })
                .await
            }
            Work::Tools => load(&sessions, &root, Load::Waking).await,
        };
        // As the session is added.
        session.report_activity();
        Self {
            root,
            _home: home,
            forge,
            sessions,
            session,
            processes,
            subagents,
        }
    }

    /// Waits until the session's clients heard `turns` of its turns start.
    async fn turns_started(&self, turns: usize) {
        let jobs = &self.sessions.jobs;
        let wait = async {
            loop {
                // Enabled before the look, so a start after it wakes this.
                let changed = jobs.changed();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let started = seen(&self.session)
                    .into_iter()
                    .filter(|seen| *seen == Seen::TurnRunning)
                    .count();
                if started >= turns {
                    return;
                }
                changed.await;
            }
        };
        timeout(TEST_BUDGET, wait)
            .await
            .expect("the daemon starts a delivery turn");
    }

    /// Queues news of the watched pull request's checks `failed`, as the
    /// poll `minutes` after the watch reads them, and cancels the delivery
    /// turn it starts, the session's `turn`th, while that turn waits for the
    /// runtime held here: it ends before it begins its prompt and leaves the
    /// news, as a delivery turn that fails first does.
    async fn news_left_by_delivery(&self, failed: &[&str], minutes: u64, turn: usize) {
        let held = self.session.runtime.lock().await;
        self.sessions
            .pull_requests
            .fail_checks_for_tests(&self.forge, failed, minutes)
            .await;
        self.turns_started(turn).await;
        self.session.cancel_turn();
        drop(held);
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

// Covers: news on a watched pull request, read through the inbox the
// session's tools hold, starts a delivery turn on the idle session, though
// no turn of its own started the watch's work; a quiet watch keeps nothing
// active; a delivery turn that left the news settles the session rather
// than loop, though every turn's end refreshes the session's tools; and
// later news wakes it again although its watcher gave up, which only the
// pull request inbox can do. A miss leaves the news unread until the user
// prompts, or starts delivery turns without end.
// Owner: daemon idle wake.
#[tokio::test]
async fn a_watched_pull_requests_news_starts_a_delivery_turn_on_the_idle_session() {
    use wire::SessionActivity::{Active, Settled};
    let fixture = Fixture::watching().await;
    let (sessions, session) = (&fixture.sessions, &fixture.session);
    let inbox = sessions
        .pull_requests
        .inbox(&session.id, fixture.root.path());
    watch_for_tests(&fixture.forge, &inbox).await;
    session.report_activity();
    let quiet = seen(session);
    sessions.watch_idle_results(session);

    let settled = || seen(session).last() == Some(&Seen::Activity(Settled));
    fixture.news_left_by_delivery(&["ci"], 1, 1).await;
    until(settled, "the watcher gives up on the news the turn left").await;
    let left = inbox.has_news();
    fixture.news_left_by_delivery(&["ci", "lint"], 2, 2).await;
    until(settled, "the watcher gives up again").await;

    assert_eq!(
        (quiet, left, seen(session)),
        (
            vec![Seen::Activity(Settled)],
            true,
            vec![
                Seen::Activity(Settled),
                Seen::Activity(Active),
                Seen::TurnRunning,
                Seen::TurnEnded,
                Seen::Activity(Settled),
                Seen::Activity(Active),
                Seen::TurnRunning,
                Seen::TurnEnded,
                Seen::Activity(Settled),
            ]
        )
    );
    fixture.shutdown().await;
}
