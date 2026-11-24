use std::path::PathBuf;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{JobRegistry, RegisterError, RunsUnder, SessionJob, StartTurnError};
use crate::app::daemon_host::jobs::{
    Admission, JobEvent, JobId, JobRecord, JobState, JobStore, TurnEnd, TurnRecord,
};

fn states(records: Vec<JobRecord>) -> Vec<(JobId, JobState)> {
    let mut states: Vec<_> = records
        .into_iter()
        .map(|record| (record.id, record.state))
        .collect();
    states.sort_by(|a, b| a.0.cmp(&b.0));
    states
}

// Covers: a daemon that starts over its predecessor's records lists every
// one, records each job that was queued, running, or waiting for input as
// interrupted, on disk as well as in memory, and leaves every other state
// as it was.
// Owner: daemon job registry startup.
#[test]
fn startup_interrupts_the_turns_a_dead_daemon_left_open() {
    use JobState::{Ended, Idle, Queued, Running, WaitingForInput};
    let home = TempDir::new().unwrap();
    let store = JobStore::open(home.path()).unwrap();
    let failed = Ended(TurnEnd::Failed("provider refused the request".into()));
    let cases = [
        (Idle, Idle),
        (Queued, Ended(TurnEnd::Interrupted)),
        (Running, Ended(TurnEnd::Interrupted)),
        (WaitingForInput, Ended(TurnEnd::Interrupted)),
        (Ended(TurnEnd::Completed), Ended(TurnEnd::Completed)),
        (failed.clone(), failed),
        (Ended(TurnEnd::Cancelled), Ended(TurnEnd::Cancelled)),
        (Ended(TurnEnd::Interrupted), Ended(TurnEnd::Interrupted)),
    ];
    let mut expected = Vec::new();
    for (saved, after) in cases {
        let record = JobRecord {
            state: saved,
            ..JobRecord::new_session(
                JobId::generate(),
                PathBuf::from("/work"),
                rho_sdk::SessionId::new(),
            )
        };
        expected.push((record.id.clone(), after));
        store.write(&record).unwrap();
    }
    expected.sort_by(|a, b| a.0.cmp(&b.0));

    let registry = JobRegistry::open(store.clone());

    assert_eq!(states(registry.list()), expected);
    assert_eq!(states(store.load().unwrap()), expected);
    assert_eq!(registry.in_progress(), 0);
}

// Covers: a turn starts only on a job with none under way, so a second
// prompt to a running, waiting, or queued session is refused as busy and
// leaves it as it was, and an unknown job is reported as unknown; the turn of
// a session admitted through the shared budget queues for capacity first
// (#1714).
// Owner: daemon job registry.
#[test]
fn a_turn_starts_only_when_none_is_in_progress() {
    let home = TempDir::new().unwrap();
    let registry = JobRegistry::open(JobStore::open(home.path()).unwrap());
    let job = JobRecord::new_session(
        JobId::generate(),
        PathBuf::from("/work"),
        rho_sdk::SessionId::new(),
    );
    let id = job.id.clone();
    let started_by = JobRecord {
        admission: Admission::SharedBudget,
        ..JobRecord::new_session(
            JobId::generate(),
            PathBuf::from("/work"),
            rho_sdk::SessionId::new(),
        )
    };
    let child = started_by.id.clone();
    registry.register(job).unwrap();
    registry.register(started_by).unwrap();

    let start = |job: &JobId| {
        registry
            .start_turn(
                job,
                TurnRecord::starting(/*key*/ None),
                crate::caller_context::CallerContext::ROOT,
            )
            .map(|(moved, _)| moved.state)
    };
    let started = start(&id);
    let while_running = start(&id);
    registry.record(&id, JobEvent::InputOpened).unwrap();
    let while_waiting = start(&id);
    let unknown = start(&JobId::generate());
    let queued = start(&child);
    let while_queued = start(&child);

    assert_eq!(
        [
            started,
            while_running,
            while_waiting,
            unknown,
            queued,
            while_queued
        ],
        [
            Ok(JobState::Running),
            Err(StartTurnError::Busy),
            Err(StartTurnError::Busy),
            Err(StartTurnError::Unknown(super::UnknownJob)),
            Ok(JobState::Queued),
            Err(StartTurnError::Busy),
        ]
    );
    assert_eq!(
        registry.get(&id).map(|record| record.state),
        Some(JobState::WaitingForInput)
    );
}

// Covers: the daemon's stop records a running job as interrupted, on disk
// too, and refuses every job registered after it, so a run started during
// the stop is never left listed as running.
// Owner: daemon job registry stop.
#[test]
fn a_closed_registry_interrupts_its_turns_and_takes_no_new_jobs() {
    let home = TempDir::new().unwrap();
    let store = JobStore::open(home.path()).unwrap();
    let registry = JobRegistry::open(store.clone());
    let job = |state| JobRecord {
        state,
        ..JobRecord::new_session(
            JobId::generate(),
            PathBuf::from("/work"),
            rho_sdk::SessionId::new(),
        )
    };
    let running = job(JobState::Running);
    let expected = vec![(running.id.clone(), JobState::Ended(TurnEnd::Interrupted))];
    registry.register(running).unwrap();

    registry.close();

    assert!(matches!(
        registry.register(job(JobState::Queued)),
        Err(RegisterError::Closed)
    ));
    assert_eq!(states(registry.list()), expected);
    assert_eq!(states(store.load().unwrap()), expected);
}

// Covers: only a restart in place marks the session turns it interrupts
// for the restarted daemon to continue, whether the turn ended interrupted
// or the closing registry interrupted it, and only a turn its record says
// may continue: never one with a time limit, or of a session whose client
// takes no daemon turns, as `rho run --detach --timeout`'s. A dead daemon's
// turn swept at the next start, a stop's, one of a restart a stop
// overrode, a turn cancelled during a restart, and a subagent run are
// never marked either. The next start takes a mark and clears it on disk,
// so the start after that, as of a daemon that died continuing the turn,
// continues nothing.
// Owner: daemon job registry stop and startup.
#[test]
fn only_a_restart_marks_the_turns_it_interrupts_to_continue() {
    use crate::daemon::Ending::{self, Restart, Stop};

    /// How the job's turn ends.
    enum Ends {
        /// Its daemon died; the next start sweeps it.
        SweptAtStart,
        /// The turn task records this end during the stop.
        Turn(TurnEnd),
        /// The stop closes the registry with the turn still under way.
        Closed,
    }
    // A session job running a turn that `continues` after a restart, as
    // its record says.
    let session_running = |continues: bool, session: &rho_sdk::SessionId| JobRecord {
        state: JobState::Running,
        turn: Some(TurnRecord {
            continues_after_restart: continues,
            ..TurnRecord::starting(/*key*/ None)
        }),
        ..JobRecord::new_session(JobId::generate(), PathBuf::from("/work"), session.clone())
    };
    let running_session = |session: &rho_sdk::SessionId| session_running(true, session);
    let running_not_continued = |session: &rho_sdk::SessionId| session_running(false, session);
    let running_subagent = |session: &rho_sdk::SessionId| JobRecord {
        state: JobState::Running,
        ..JobRecord::new_subagent(
            JobId::generate(),
            PathBuf::from("/work"),
            Some(session.clone()),
            "run".into(),
        )
    };
    let interrupted = || Ends::Turn(TurnEnd::Interrupted);
    /// The job a case runs, for its session.
    type Job<'a> = &'a dyn Fn(&rho_sdk::SessionId) -> JobRecord;
    // Name, job, the endings asked for in order, how the turn ends, and
    // whether the next start continues it.
    let cases: [(&str, Job<'_>, &[Ending], Ends, bool); 9] = [
        (
            "dead daemon",
            &running_session,
            &[],
            Ends::SweptAtStart,
            false,
        ),
        ("stop", &running_session, &[Stop], interrupted(), false),
        (
            "stop, still running",
            &running_session,
            &[Stop],
            Ends::Closed,
            false,
        ),
        ("restart", &running_session, &[Restart], interrupted(), true),
        (
            "restart, still running",
            &running_session,
            &[Restart],
            Ends::Closed,
            true,
        ),
        (
            "restart, then stop",
            &running_session,
            &[Restart, Stop],
            interrupted(),
            false,
        ),
        (
            "restart, cancelled",
            &running_session,
            &[Restart],
            Ends::Turn(TurnEnd::Cancelled),
            false,
        ),
        (
            "restart, turn that may not continue",
            &running_not_continued,
            &[Restart],
            Ends::Closed,
            false,
        ),
        (
            "restart, subagent run",
            &running_subagent,
            &[Restart],
            Ends::Closed,
            false,
        ),
    ];
    // A daemon running `job`, whose stop was asked for as `endings` say, in
    // order.
    let stopping = |store: &JobStore, job: JobRecord, endings: &[Ending]| {
        let registry = JobRegistry::open(store.clone());
        registry.register(job).unwrap();
        for ending in endings {
            registry.stopping_for(*ending);
        }
        registry
    };
    let mut taken = Vec::new();
    let mut expected = Vec::new();
    for (name, job, endings, ends, marked) in cases {
        let home = TempDir::new().unwrap();
        let store = JobStore::open(home.path()).unwrap();
        let session = rho_sdk::SessionId::new();
        let job = job(&session);
        let id = job.id.clone();
        match ends {
            Ends::SweptAtStart => store.write(&job).unwrap(),
            Ends::Turn(end) => {
                let registry = stopping(&store, job, endings);
                registry.end_turn(&id, end, None).unwrap();
            }
            Ends::Closed => stopping(&store, job, endings).close(),
        }

        let next_start = JobRegistry::open(store.clone()).take_restart_interrupted();
        let start_after = JobRegistry::open(store).take_restart_interrupted();

        taken.push((name, next_start, start_after));
        expected.push((
            name,
            if marked { vec![session] } else { Vec::new() },
            Vec::new(),
        ));
    }
    assert_eq!(taken, expected);
}

// Covers: an upgrade closes the registry only while no job is in progress,
// and not after a turn that ran since it found the daemon idle, which may
// have left work its other checks missed; once closed, no turn starts, so a
// prompt that races the upgrade is refused rather than interrupted (#1713).
// Owner: daemon job registry upgrade.
#[test]
fn an_upgrade_closes_the_registry_only_while_idle() {
    let home = TempDir::new().unwrap();
    let registry = JobRegistry::open(JobStore::open(home.path()).unwrap());
    let job = JobRecord::new_session(
        JobId::generate(),
        PathBuf::from("/work"),
        rho_sdk::SessionId::new(),
    );
    let id = job.id.clone();
    registry.register(job).unwrap();

    let before = registry.idle().unwrap();
    registry
        .start_turn(
            &id,
            TurnRecord::starting(/*key*/ None),
            crate::caller_context::CallerContext::ROOT,
        )
        .unwrap();
    let while_running = registry.idle();
    registry
        .record(&id, JobEvent::TurnEnded(TurnEnd::Completed))
        .unwrap();
    // A turn that ran since the daemon was found idle may have left work.
    let after_a_turn = registry.close_if_still(before);
    let idle = registry.idle().unwrap();
    let still_idle = registry.close_if_still(idle);
    let next_turn = registry
        .start_turn(
            &id,
            TurnRecord::starting(/*key*/ None),
            crate::caller_context::CallerContext::ROOT,
        )
        .map(|(moved, _)| moved.state);

    assert_eq!(
        (while_running, after_a_turn, still_idle, next_turn),
        (None, false, true, Err(StartTurnError::Closed))
    );
}

// Covers: what a reopen finds for a session (#1661): nothing when no job ran
// its turns (another session's, or a finished subagent run that wrote it,
// does not count); the latest of its session jobs otherwise; and the job
// that writes it while one has a turn under way, a subagent run's included,
// so the session is not opened twice.
// Owner: daemon job registry session lookup.
#[test]
fn a_session_reopens_on_its_latest_job_unless_one_writes_it() {
    let session = rho_sdk::SessionId::new();
    let turns = |created_at, state| JobRecord {
        state,
        created_at,
        ..JobRecord::new_session(JobId::generate(), PathBuf::from("/work"), session.clone())
    };
    let subagent = |state| JobRecord {
        state,
        ..JobRecord::new_subagent(
            JobId::generate(),
            PathBuf::from("/work"),
            Some(session.clone()),
            "run".into(),
        )
    };
    let other = JobRecord::new_session(
        JobId::generate(),
        PathBuf::from("/work"),
        rho_sdk::SessionId::new(),
    );
    let older = turns(1, JobState::Ended(TurnEnd::Completed));
    let newer = turns(2, JobState::Ended(TurnEnd::Interrupted));
    let running = turns(3, JobState::Running);
    let writing = subagent(JobState::Running);
    let wrote = subagent(JobState::Ended(TurnEnd::Completed));
    let cases = [
        ("nothing", vec![], SessionJob::New),
        ("another session", vec![other.clone()], SessionJob::New),
        ("a finished subagent", vec![wrote.clone()], SessionJob::New),
        (
            "two session jobs",
            vec![newer.clone(), older.clone(), wrote],
            SessionJob::Last(Box::new(newer.clone())),
        ),
        (
            "a running turn",
            vec![older.clone(), running.clone()],
            SessionJob::Busy(running.id.clone()),
        ),
        (
            "a running subagent",
            vec![newer, other, writing.clone()],
            SessionJob::Busy(writing.id.clone()),
        ),
    ];
    for (name, records, expected) in cases {
        let home = TempDir::new().unwrap();
        let registry = JobRegistry::open(JobStore::open(home.path()).unwrap());
        for record in records {
            registry.register(record).unwrap();
        }
        assert_eq!(registry.session_job(&session), expected, "{name}");
    }
}

// Covers: a session job's record holds the grant of its turn under way, each
// turn's replacing the last one's, and a daemon that starts over the records
// still finds it (#1780).
// Owner: daemon job registry, session grants.
#[test]
fn a_turns_grant_is_kept_on_its_jobs_record_across_daemons() {
    use crate::caller_context::{Access, CallerContext, Scope, Workspaces};
    let home = TempDir::new().unwrap();
    let registry = JobRegistry::open(JobStore::open(home.path()).unwrap());
    let job = JobRecord::new_session(
        JobId::generate(),
        PathBuf::from("/work"),
        rho_sdk::SessionId::new(),
    );
    let id = job.id.clone();
    registry.register(job).unwrap();
    let limited = CallerContext {
        depth: 2,
        scope: Scope {
            access: Access::Read,
            workspaces: Workspaces::Only(vec![PathBuf::from("/work")]),
        },
    };

    let opened = registry.grant_of(&id);
    registry
        .start_turn(&id, TurnRecord::starting(/*key*/ None), limited.clone())
        .unwrap();
    let under_way = registry.grant_of(&id);
    drop(registry);
    let restarted = JobRegistry::open(JobStore::open(home.path()).unwrap());

    assert_eq!(
        (opened, under_way, restarted.grant_of(&id)),
        (
            Some(CallerContext::ROOT),
            Some(limited.clone()),
            Some(limited)
        )
    );
}

// Covers: a job with a grant limit, as a session an agent started has, never records more than the limit: a turn that asks for no limit, as
// a person's prompt does, gets the limit; one that asks for less gets less;
// a reopen is held the same; a daemon that starts over the records still
// holds it; and a job without a limit takes each grant as asked (#1796).
// Owner: daemon job registry, session grants.
#[test]
fn a_jobs_grant_never_exceeds_its_limit() {
    use crate::caller_context::{Access, CallerContext, Scope, Workspaces};
    let only = |root: &str| Workspaces::Only(vec![PathBuf::from(root)]);
    let limit = CallerContext {
        depth: 2,
        scope: Scope {
            access: Access::Write,
            workspaces: only("/work"),
        },
    };
    let narrower = CallerContext {
        depth: 3,
        scope: Scope {
            access: Access::Read,
            workspaces: only("/work/deep"),
        },
    };
    let home = TempDir::new().unwrap();
    let registry = JobRegistry::open(JobStore::open(home.path()).unwrap());
    let session = || {
        JobRecord::new_session(
            JobId::generate(),
            PathBuf::from("/work"),
            rho_sdk::SessionId::new(),
        )
    };
    let child = JobRecord {
        grant: limit.clone(),
        grant_limit: Some(limit.clone()),
        ..session()
    };
    let top_level = session();
    let (child, top_level) = {
        let ids = (child.id.clone(), top_level.id.clone());
        registry.register(child).unwrap();
        registry.register(top_level).unwrap();
        ids
    };
    let turn = |registry: &JobRegistry, job: &JobId, asked: &CallerContext| {
        let (_, granted) = registry
            .start_turn(job, TurnRecord::starting(/*key*/ None), asked.clone())
            .unwrap();
        registry
            .record(job, JobEvent::TurnEnded(TurnEnd::Completed))
            .unwrap();
        (granted, registry.grant_of(job).unwrap())
    };

    let by_a_person = turn(&registry, &child, &CallerContext::ROOT);
    let asking_less = turn(&registry, &child, &narrower);
    registry.set_grant(&child, CallerContext::ROOT);
    let reopened = registry.grant_of(&child).unwrap();
    let unlimited = turn(&registry, &top_level, &CallerContext::ROOT);
    drop(registry);
    let restarted = JobRegistry::open(JobStore::open(home.path()).unwrap());
    let after_a_restart = turn(&restarted, &child, &CallerContext::ROOT);

    assert_eq!(
        (
            by_a_person,
            asking_less,
            reopened,
            unlimited,
            after_a_restart
        ),
        (
            (limit.clone(), limit.clone()),
            (narrower.clone(), narrower),
            limit.clone(),
            (CallerContext::ROOT, CallerContext::ROOT),
            (limit.clone(), limit),
        )
    );
}

// Covers: a session started from a subagent's or a workflow node's session
// runs under the session whose turn runs them: it is held under that
// session's grant, not the unlimited one their own jobs keep (#1796), and
// lists that session as the one that started it; a run whose session job
// the daemon does not know runs under none.
// Owner: daemon job registry, session grants and starters.
#[test]
fn a_run_runs_under_the_session_whose_turn_runs_it() {
    use crate::caller_context::{Access, CallerContext, Scope, Workspaces};
    let home = TempDir::new().unwrap();
    let registry = JobRegistry::open(JobStore::open(home.path()).unwrap());
    let limited = CallerContext {
        depth: 1,
        scope: Scope {
            access: Access::Read,
            workspaces: Workspaces::Any,
        },
    };
    let lead = rho_sdk::SessionId::new();
    let session = JobRecord {
        grant: limited.clone(),
        ..JobRecord::new_session(JobId::generate(), PathBuf::from("/work"), lead.clone())
    };
    let workflow = JobRecord::new_workflow(
        session.id.clone(),
        PathBuf::from("/work"),
        crate::workflow::RunId::new(),
    );
    let subagent =
        |parent: JobId| JobRecord::new_subagent(parent, PathBuf::from("/work"), None, "run".into());
    let run = subagent(session.id.clone());
    let node = subagent(workflow.id.clone());
    let orphan = subagent(JobId::generate());
    let ids = [&session, &workflow, &run, &node, &orphan].map(|record| record.id.clone());
    for record in [session, workflow, run, node, orphan] {
        registry.register(record).unwrap();
    }

    let under_lead = Some(RunsUnder {
        session: lead,
        grant: limited,
    });
    assert_eq!(
        ids.map(|id| registry.runs_under(&id)),
        [
            under_lead.clone(),
            under_lead.clone(),
            under_lead.clone(),
            under_lead,
            None
        ]
    );
}
