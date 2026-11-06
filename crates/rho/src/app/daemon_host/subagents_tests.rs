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
        daemon_host::jobs::{JobEvent, JobId, JobRegistry, JobState, JobStore, TurnEnd},
        subagent_pool::{StartedSubagent, SubagentPool},
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
            Some(JobEvent::Admitted),
        ),
        (
            Queued,
            status(RunState::Running),
            false,
            Some(JobEvent::Admitted),
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

// Covers: a run's job reports where the run waits in the daemon-wide
// budget, whose limit the opened session's config sets, until the run has
// capacity; a run the daemon's stop ends is recorded interrupted, not
// cancelled.
// Owner: daemon subagent jobs.
#[tokio::test(flavor = "current_thread")]
async fn a_child_job_reports_its_queue_place_until_it_runs() {
    let home = TempDir::new().unwrap();
    let registry = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    let stopping = CancellationToken::new();
    let subagents = Subagents::new(Arc::clone(&registry), stopping.clone());
    subagents.set_limit(&Config {
        agent_concurrency: 1,
        ..Config::default()
    });
    let SubagentPool::Shared {
        concurrency,
        observer,
    } = subagents.pool_for(JobId::generate())
    else {
        panic!("the daemon shares its pool");
    };
    let held = concurrency
        .join(CapacityClass::Rho)
        .acquire(&RunCancellation::new())
        .await
        .expect("a run of another session holds the only slot");
    let queued = concurrency.join(CapacityClass::Rho);
    let (status_tx, status_rx) = watch::channel(status(RunState::Queued));
    observer.started(StartedSubagent {
        run_id: "run-1".into(),
        session: Some(rho_sdk::SessionId::new()),
        workspace: PathBuf::from("/work"),
        status: status_rx,
        queue: queued.watch(),
    });
    let job = registry.list().pop().expect("the run's job").id;
    assert_eq!(
        subagents.queue_place(&job),
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
    wait_for_state(&registry, &job, JobState::Running).await;
    assert_eq!(subagents.queue_place(&job), None);

    stopping.cancel();
    status_tx.send_replace(status(RunState::Stopped));
    wait_for_state(&registry, &job, JobState::Ended(TurnEnd::Interrupted)).await;
}
