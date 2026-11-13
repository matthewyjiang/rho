use std::{path::PathBuf, sync::Arc};

use pretty_assertions::assert_eq;
use rho_sdk::CancellationToken;
use rho_tools::cancellation::RunCancellation;
use tempfile::TempDir;
use tokio::sync::watch;

use super::{next_event, workflow_end, Subagents};
use crate::{
    app::{
        agent_binding::CapacityClass,
        agent_concurrency::{Budget, QueuePlace},
        daemon_host::jobs::{JobEvent, JobId, JobRegistry, JobState, JobStore, TurnEnd},
        subagent_pool::{StartedSubagent, SubagentPool},
    },
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

// Covers: a workflow drive that stopped without an end was interrupted
// when the daemon is stopping, and failed otherwise.
// Owner: daemon workflow jobs.
#[test]
fn a_workflow_drive_without_an_end_is_interrupted_only_by_a_stop() {
    assert_eq!(
        (workflow_end(None, true), workflow_end(None, false)),
        (
            TurnEnd::Interrupted,
            TurnEnd::Failed("the workflow run stopped without an end".into()),
        )
    );
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
    subagents.concurrency().set_total(1);
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
    observer.subagent_started(StartedSubagent {
        run_id: "run-1".into(),
        session: Some(rho_sdk::SessionId::new()),
        workspace: PathBuf::from("/work"),
        status: status_rx,
        queue: queued.watch(),
        events: None,
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

// Covers: the daemon follows a run once the run opened its session, with
// the model the session runs on, and takes the run's title from its status
// whether the title came before the session opened or after; a run that
// ended is followed no more.
// Owner: daemon subagent jobs.
#[tokio::test(flavor = "current_thread")]
async fn a_followed_run_lists_its_model_and_title() {
    let home = TempDir::new().unwrap();
    let registry = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    let subagents = Subagents::new(Arc::clone(&registry), CancellationToken::new());
    let SubagentPool::Shared { observer, .. } = subagents.pool_for(JobId::generate()) else {
        panic!("the daemon shares its pool");
    };
    let model = rho_protocol::ModelRef {
        provider: "xai".into(),
        model: "grok-fixture".into(),
    };
    let options = rho_protocol::SessionOptions {
        model: rho_protocol::ModelOptions {
            current: model.clone(),
            auth: "api-key".into(),
            available: Vec::new(),
        },
        reasoning: None,
        fast_mode: None,
        permission_mode: rho_protocol::PermissionModeOptions {
            current: rho_protocol::PermissionMode::Auto,
            available: Vec::new(),
        },
        commands: Vec::new(),
        added_dirs: Vec::new(),
    };
    let titled = |title: &str| RunStatus {
        title: Some(title.into()),
        ..status(RunState::Running)
    };
    let (status_tx, status_rx) = watch::channel(titled("Early title"));
    let (events, followed) = tokio::sync::mpsc::unbounded_channel();
    let session = rho_sdk::SessionId::new();
    let queued = subagents.concurrency().join(CapacityClass::Rho);
    observer.subagent_started(StartedSubagent {
        run_id: "run-1".into(),
        session: Some(session.clone()),
        workspace: PathBuf::from("/work"),
        status: status_rx,
        queue: queued.watch(),
        events: Some(followed),
    });
    let listed = |title: Option<&'static str>| {
        let (feeds, session) = (subagents.feeds().clone(), session.clone());
        async move {
            for _ in 0..10_000 {
                let live = feeds.live(&session);
                if live.as_ref().map(|live| live.title.as_deref()) == Some(title) {
                    return live.map(|live| live.model);
                }
                tokio::task::yield_now().await;
            }
            panic!(
                "the run never listed title {title:?}: {:?}",
                feeds.live(&session)
            );
        }
    };
    let unopened = subagents.feeds().live(&session);

    events
        .send(crate::app::subagent_pool::ChildEvent::Opened(Box::new(
            options,
        )))
        .unwrap();
    let early = listed(Some("Early title")).await;
    status_tx.send_replace(titled("Late title"));
    let late = listed(Some("Late title")).await;
    status_tx.send_replace(status(RunState::Ok));
    let job = registry.list().pop().expect("the run's job").id;
    wait_for_state(&registry, &job, JobState::Ended(TurnEnd::Completed)).await;
    for _ in 0..10_000 {
        if subagents.feeds().live(&session).is_none() {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert_eq!(
        (unopened, early, late, subagents.feeds().live(&session)),
        (None, Some(model.clone()), Some(model), None)
    );
}
