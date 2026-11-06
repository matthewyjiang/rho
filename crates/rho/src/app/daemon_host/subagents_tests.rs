use std::{path::PathBuf, sync::Arc};

use pretty_assertions::assert_eq;
use rho_sdk::CancellationToken;
use rho_tools::cancellation::RunCancellation;
use tempfile::TempDir;
use tokio::sync::watch;

use super::{next_event, Subagents};
use crate::{
    app::{
        agent_binding::CapacityClass,
        agent_concurrency::{Budget, QueuePlace},
        daemon_host::jobs::{
            JobEvent, JobId, JobKind, JobRecord, JobRegistry, JobState, JobStore, TurnEnd,
        },
        subagent_host::{StartedSubagent, SubagentHost},
    },
    config::Config,
    subagent::{RunState, RunStatus},
};

fn status(state: RunState) -> RunStatus {
    RunStatus {
        state,
        ..RunStatus::default()
    }
}

// Covers: a subagent job starts when its run has capacity, ends as its run
// ends (with the run's error when it failed), and a run stopped while the
// daemon stops is interrupted rather than cancelled; a job already where
// its run is takes no event.
// Owner: daemon subagent jobs.
#[test]
fn a_subagent_job_follows_its_run() {
    use JobState::{Queued, Running};
    use TurnEnd::{Cancelled, Completed, Failed, Interrupted};
    let failed = RunStatus {
        error: Some("provider refused the request".into()),
        ..status(RunState::Error)
    };
    let ended = |end| Some(JobEvent::TurnEnded(end));
    let cases = [
        (Queued, status(RunState::Queued), false, None),
        (
            Queued,
            status(RunState::Starting),
            false,
            Some(JobEvent::TurnStarted),
        ),
        (
            Queued,
            status(RunState::Running),
            false,
            Some(JobEvent::TurnStarted),
        ),
        (Running, status(RunState::Running), false, None),
        (Running, status(RunState::Ok), false, ended(Completed)),
        (
            Running,
            failed.clone(),
            false,
            ended(Failed("provider refused the request".into())),
        ),
        (
            Running,
            status(RunState::Error),
            false,
            ended(Failed("the subagent run failed".into())),
        ),
        (Running, status(RunState::Stopped), false, ended(Cancelled)),
        (Running, status(RunState::Stopped), true, ended(Interrupted)),
        (Queued, status(RunState::Stopped), false, ended(Cancelled)),
        (
            JobState::Ended(Interrupted),
            status(RunState::Stopped),
            true,
            None,
        ),
    ];
    for (state, run, stopping, expected) in cases {
        assert_eq!(
            next_event(&state, &run, stopping),
            expected,
            "{state} with the run {:?}, stopping: {stopping}",
            run.state
        );
    }
}

/// Yields until job `id` is in `state`.
async fn wait_for_state(registry: &JobRegistry, id: &JobId, state: JobState) {
    for _ in 0..10_000 {
        if registry.get(id).is_some_and(|record| record.state == state) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("job {id} never reached {state}: {:?}", registry.get(id));
}

// Covers: a run a session starts is recorded as a queued child job of the
// session's job, with the session it writes and where it waits in the
// daemon-wide budget, whose limit is the opened session's config; the job
// then moves with the run to running and completed, and a run the daemon's
// stop ends is recorded interrupted.
// Owner: daemon subagent jobs.
#[tokio::test(flavor = "current_thread")]
async fn a_session_s_runs_are_child_jobs_of_its_job() {
    let home = TempDir::new().unwrap();
    let registry = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    let stopping = CancellationToken::new();
    let subagents = Subagents::new(Arc::clone(&registry), stopping.clone());
    let parent = JobId::generate();
    let config = Config {
        agent_concurrency: 1,
        ..Config::default()
    };
    let SubagentHost::Shared {
        concurrency,
        observer,
    } = subagents.host_for(parent.clone(), &config)
    else {
        panic!("the daemon shares its pool");
    };
    let held = concurrency
        .join(CapacityClass::Rho)
        .acquire(&RunCancellation::new())
        .await
        .expect("a run of another session holds the only slot");

    let child_session = rho_sdk::SessionId::new();
    let queued = concurrency.join(CapacityClass::Rho);
    let (status_tx, status_rx) = watch::channel(status(RunState::Queued));
    observer.started(StartedSubagent {
        run_id: "run-1".into(),
        session: Some(child_session.clone()),
        workspace: PathBuf::from("/work"),
        status: status_rx,
        queue: queued.watch(),
    });
    let jobs = registry.list();
    let [child] = jobs.as_slice() else {
        panic!("one job: {jobs:?}");
    };
    assert_eq!(
        child,
        &JobRecord {
            kind: JobKind::Subagent,
            state: JobState::Queued,
            workspace: PathBuf::from("/work"),
            parent: Some(parent.clone()),
            session: Some(child_session),
            ..child.clone()
        }
    );
    assert_eq!(
        subagents.queue_place(&child.id),
        Some(QueuePlace {
            budget: Budget::AgentConcurrency,
            limit: 1,
            position: 1,
        })
    );

    drop(held);
    let _permits = queued
        .acquire(&RunCancellation::new())
        .await
        .expect("capacity");
    status_tx.send_replace(status(RunState::Running));
    wait_for_state(&registry, &child.id, JobState::Running).await;
    assert_eq!(subagents.queue_place(&child.id), None);
    status_tx.send_replace(status(RunState::Ok));
    wait_for_state(&registry, &child.id, JobState::Ended(TurnEnd::Completed)).await;

    let (status_tx, status_rx) = watch::channel(status(RunState::Running));
    observer.started(StartedSubagent {
        run_id: "run-2".into(),
        session: None,
        workspace: PathBuf::from("/work"),
        status: status_rx,
        queue: Default::default(),
    });
    let second = registry
        .list()
        .into_iter()
        .find(|record| record.id != child.id)
        .expect("the second run's job")
        .id;
    wait_for_state(&registry, &second, JobState::Running).await;
    stopping.cancel();
    status_tx.send_replace(status(RunState::Stopped));
    wait_for_state(&registry, &second, JobState::Ended(TurnEnd::Interrupted)).await;
}
