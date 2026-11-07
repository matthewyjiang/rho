use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::{
    app::{
        daemon_host::{
            jobs::{JobId, JobRecord, JobRegistry, JobStore},
            sessions::{open::OpenedSession, LoadedSession, Sessions},
        },
        session_runtime::{unattended_runtime_on, HangUntilCancel, QuestionSupport},
    },
    config::Config,
};

const GRACE: Duration = Duration::from_secs(60);

/// A session loaded in `sessions` on a new job, its turn started when
/// `busy`, with a provider that never answers.
async fn load(sessions: &Sessions, root: &TempDir, busy: bool) -> Arc<LoadedSession> {
    let config = Config::default();
    let provider: Arc<dyn rho_sdk::provider::ModelProvider> = Arc::new(HangUntilCancel);
    let sdk = rho_sdk::Rho::builder()
        .provider_shared(Arc::clone(&provider))
        .build()
        .unwrap();
    let runtime = unattended_runtime_on(
        root.path(),
        &config,
        QuestionSupport::Unavailable,
        sdk,
        provider,
    )
    .await;
    let id = runtime.session_id().clone();
    let record = JobRecord::new_session(JobId::generate(), PathBuf::from("/work"), id.clone());
    let job = record.id.clone();
    sessions.jobs.register(record).unwrap();
    if busy {
        sessions.jobs.start_turn(&job).unwrap();
    }
    let session = Arc::new(LoadedSession::new(
        &sessions.jobs.get(&job).unwrap(),
        Arc::clone(&sessions.jobs),
        OpenedSession { runtime, config },
    ));
    sessions.loaded().insert(id, Arc::clone(&session));
    session
}

/// The jobs of the sessions loaded in `sessions`.
fn loaded_jobs(sessions: &Sessions) -> BTreeSet<String> {
    sessions
        .loaded()
        .values()
        .map(|session| session.job.to_string())
        .collect()
}

// Covers: sweeps less than a grace apart only mark an idle session idle;
// one a grace after the first unloads it, tearing its runtime down and turning away requests that found
// it before, while a session with a turn under way, one a client follows,
// and one a request used since the first sweep stay loaded (#1661).
// Owner: daemon idle session unload (unit).
#[tokio::test]
async fn a_session_idle_for_the_grace_is_unloaded_and_no_other() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    let sessions = Sessions::new(jobs);
    let busy = load(&sessions, &root, /*busy*/ true).await;
    let followed = load(&sessions, &root, /*busy*/ false).await;
    let _follower = followed.feed.attach(None);
    let used = load(&sessions, &root, /*busy*/ false).await;
    let idle = load(&sessions, &root, /*busy*/ false).await;
    let everything = loaded_jobs(&sessions);

    sessions.sweep(GRACE).await;
    sessions.sweep(GRACE).await;
    let within_the_grace = loaded_jobs(&sessions);
    drop(used.enter());
    tokio::time::pause();
    tokio::time::advance(GRACE).await;
    tokio::time::resume();
    sessions.sweep(GRACE).await;

    let kept = [&busy, &followed, &used]
        .into_iter()
        .map(|session| session.job.to_string())
        .collect();
    assert_eq!(
        (
            within_the_grace,
            loaded_jobs(&sessions),
            idle.runtime.lock().await.is_none(),
            idle.enter().is_none(),
        ),
        (everything, kept, true, true)
    );
}
