use std::{collections::BTreeSet, sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use crate::app::daemon_host::{
    jobs::{JobRecord, JobRegistry, JobStore},
    sessions::{
        test_sessions::{load, Load},
        LoadedSession, Sessions,
    },
};

const GRACE: Duration = Duration::from_secs(60);

/// The jobs of `sessions`.
fn jobs_of<'a>(sessions: impl IntoIterator<Item = &'a Arc<LoadedSession>>) -> BTreeSet<String> {
    sessions
        .into_iter()
        .map(|session| session.job.to_string())
        .collect()
}

/// The ids of `records`.
fn ids(records: Vec<JobRecord>) -> BTreeSet<String> {
    records
        .into_iter()
        .map(|record| record.id.to_string())
        .collect()
}

// Covers: sweeps less than a grace apart only mark an idle session idle;
// one a grace after the first unloads a saved one, tearing its runtime down
// and turning away requests that found it before, and discards one that
// has saved nothing the same way, its job gone. A session with a turn under way, one a client follows, saved
// or not, and one a request used since the first sweep stay loaded (#1661).
// Owner: daemon idle session unload (unit).
#[tokio::test]
async fn a_session_idle_for_the_grace_leaves_and_no_other() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let jobs = Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap()));
    let sessions = Sessions::new(jobs);
    let busy = load(&sessions, &root, Load::Busy).await;
    let followed = load(&sessions, &root, Load::Saved).await;
    let _follower = followed.feed.attach(None);
    let followed_unsaved = load(&sessions, &root, Load::Unsaved).await;
    let _unsaved_follower = followed_unsaved.feed.attach(None);
    let used = load(&sessions, &root, Load::Saved).await;
    let unsaved = load(&sessions, &root, Load::Unsaved).await;
    let idle = load(&sessions, &root, Load::Saved).await;
    let everything = jobs_of(sessions.loaded().values());

    sessions.sweep(GRACE).await;
    sessions.sweep(GRACE).await;
    let within_the_grace = jobs_of(sessions.loaded().values());
    drop(used.enter());
    tokio::time::pause();
    tokio::time::advance(GRACE).await;
    tokio::time::resume();
    sessions.sweep(GRACE).await;
    let mut torn_down = Vec::new();
    for session in [&idle, &unsaved] {
        torn_down.push((
            session.runtime.lock().await.is_none(),
            session.enter().is_none(),
        ));
    }

    let kept = jobs_of([&busy, &followed, &followed_unsaved, &used]);
    let recorded = jobs_of([&busy, &followed, &followed_unsaved, &used, &idle]);
    assert_eq!(
        (
            within_the_grace,
            jobs_of(sessions.loaded().values()),
            torn_down,
            ids(sessions.jobs.list()),
        ),
        (everything, kept, vec![(true, true); 2], recorded)
    );
}
