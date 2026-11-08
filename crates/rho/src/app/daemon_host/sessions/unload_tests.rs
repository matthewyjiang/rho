use std::{collections::BTreeSet, sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::app::daemon_host::{
    jobs::{JobRegistry, JobStore},
    sessions::{
        test_sessions::{load, Load},
        Sessions,
    },
};

const GRACE: Duration = Duration::from_secs(60);

/// The jobs of the sessions loaded in `sessions`.
fn loaded_jobs(sessions: &Sessions) -> BTreeSet<String> {
    sessions
        .loaded()
        .values()
        .map(|session| session.job.to_string())
        .collect()
}

// Covers: sweeps less than a grace apart only mark an idle session idle;
// one a grace after the first unloads it, tearing its runtime down and
// turning away requests that found it before. A session with a turn under
// way, one a client follows, one a request used since the first sweep, and
// one that has saved nothing yet stay loaded (#1661).
// Owner: daemon idle session unload (unit).
#[tokio::test]
async fn a_session_idle_for_the_grace_is_unloaded_and_no_other() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    let sessions = Sessions::new(jobs);
    let busy = load(&sessions, &root, Load::Busy).await;
    let followed = load(&sessions, &root, Load::Saved).await;
    let _follower = followed.feed.attach(None);
    let used = load(&sessions, &root, Load::Saved).await;
    let unsaved = load(&sessions, &root, Load::Unsaved).await;
    let idle = load(&sessions, &root, Load::Saved).await;
    let everything = loaded_jobs(&sessions);

    sessions.sweep(GRACE).await;
    sessions.sweep(GRACE).await;
    let within_the_grace = loaded_jobs(&sessions);
    drop(used.enter());
    tokio::time::pause();
    tokio::time::advance(GRACE).await;
    tokio::time::resume();
    sessions.sweep(GRACE).await;

    let kept = [&busy, &followed, &used, &unsaved]
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
