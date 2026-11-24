use std::{sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, ContentBlock as WireBlock, PromptAuthor, SessionPromptParams};
use rho_sdk::{
    model::{ContentBlock, ModelIdentity, ModelResponse},
    provider::{ScriptedProvider, ScriptedTurn},
};
use tempfile::TempDir;
use tokio::time::timeout;

use super::{
    super::{
        pull_request_wake::watch_for_tests,
        test_sessions::{load_on, Load},
        LoadedSession, Sessions,
    },
    after_merge,
};
use crate::{
    app::{
        daemon_host::{
            jobs::{JobRegistry, JobStore},
            wire::session_to_wire,
        },
        pull_requests::{PullRequestInbox, ScriptedForge},
    },
    session::{Session as StoredSession, SessionListing, SessionMark, SessionSummary},
};

/// Bounds each wait on the daemon's own tasks; never the way a test syncs.
const TEST_BUDGET: Duration = Duration::from_secs(10);

// Covers: a merged pull request marks its session done as of now, also
// one done or kept before the merge, or whose snooze has ended; a pinned
// session stays pinned, a snoozed one snoozed until its snooze ends, and
// one the person put back since the merge stays put back, so the marking
// at the end of the turn that told the agent undoes no restore made while
// it ran. Owner: daemon session marks.
#[test]
fn a_merge_marks_a_session_done_unless_the_person_set_it_apart() {
    use SessionMark::{Done, Kept, Pinned, Snoozed};
    const MERGED_AT: u64 = 1_000;
    const NOW: u64 = 1_060;
    let done = Some(Done { at: NOW });
    let cases = [
        (None, done),
        (Some(Done { at: 900 }), done),
        (Some(Kept { at: MERGED_AT - 1 }), done),
        (Some(Snoozed { until: NOW }), done),
        (Some(Pinned { at: 900 }), Some(Pinned { at: 900 })),
        (
            Some(Snoozed { until: NOW + 1 }),
            Some(Snoozed { until: NOW + 1 }),
        ),
        (Some(Kept { at: MERGED_AT }), Some(Kept { at: MERGED_AT })),
    ];
    let marked: Vec<_> = cases
        .iter()
        .map(|(mark, _)| after_merge(*mark, MERGED_AT, NOW))
        .collect();
    assert_eq!(marked, cases.map(|(_, after)| after));
}

/// What the agent answers the turn that tells it of the merge.
const NOTED: &str = "noted";

/// What a session's clients heard of its mark and its turns, in order.
#[derive(Debug, PartialEq)]
enum Heard {
    TurnRunning,
    Mark(Option<wire::SessionMark>),
    /// The turn ended with this answer.
    Answered(String),
    /// The turn ended some other way.
    Unanswered(wire::TurnOutcome),
}

/// Which turn tells a session's agent news that waits for it.
enum Told {
    /// One the daemon starts as the news comes, the session idle.
    ByTheDaemon,
    /// The next the person prompts: the daemon starts none.
    AtTheNextPrompt,
}

/// A session loaded in its own daemon and saved under that daemon's Rho
/// home, its first prompt answered, whose agent watches a pull request
/// `forge` scripts and answers one turn more with [`NOTED`].
struct Fixture {
    /// The home's `sessions` directory, also the session's workspace.
    root: TempDir,
    _home: TempDir,
    forge: Arc<ScriptedForge>,
    sessions: Arc<Sessions>,
    session: Arc<LoadedSession>,
    /// How many events the session's feed held once it watched.
    told_before: usize,
}

impl Fixture {
    async fn watching(told: Told) -> Self {
        let home = TempDir::new().unwrap();
        let root = tempfile::Builder::new()
            .prefix("sessions")
            .rand_bytes(0)
            .tempdir_in(home.path())
            .unwrap();
        let forge = ScriptedForge::new();
        let sessions = Sessions::new(
            Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
            home.path(),
            forge.clone(),
        );
        let answers = ["hello", NOTED].map(|text| {
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::Text(
                text.into(),
            )]))
        });
        let provider = ScriptedProvider::new(ModelIdentity::new("test", "test", "model"), answers);
        let session = load_on(&sessions, &root, Load::Waking, Arc::new(provider)).await;
        // As the session is added.
        session.report_activity();
        match told {
            Told::ByTheDaemon => sessions.watch_idle_results(&session),
            Told::AtTheNextPrompt => {}
        }
        let mut fixture = Self {
            root,
            _home: home,
            forge,
            sessions,
            session,
            told_before: 0,
        };
        // A turn that tells of news continues a conversation: give it one.
        fixture.prompt().await;
        watch_for_tests(&fixture.forge, &fixture.inbox()).await;
        fixture.told_before = fixture.session.feed.buffered_for_tests().len();
        fixture
    }

    fn inbox(&self) -> PullRequestInbox {
        let (id, root) = (&self.session.id, self.root.path());
        self.sessions.pull_requests.inbox(id, root)
    }

    /// The person prompts the session, and the turn runs to its end.
    async fn prompt(&self) {
        let prompted = self.sessions.prompt(SessionPromptParams {
            author: PromptAuthor::Person,
            session: session_to_wire(self.session.id.clone()),
            content: vec![WireBlock::Text {
                text: "hello".into(),
            }],
            timeout_ms: None,
            environment: std::iter::empty::<(String, String)>().collect(),
            idempotency_key: None,
            grant: None,
        });
        timeout(TEST_BUDGET, prompted)
            .await
            .expect("the turn saves its prompt")
            .unwrap();
        // The turn holds the runtime until its end is recorded.
        let ended = timeout(TEST_BUDGET, self.session.runtime.lock());
        drop(ended.await.expect("the turn ends"));
    }

    /// The poll reads the watched pull request merged.
    async fn merge(&self) {
        let wake = &self.sessions.pull_requests;
        wake.merge_for_tests(&self.forge, /*minutes*/ 1).await;
    }

    /// The watched pull request merges, and the delivery turn its news
    /// starts on the idle session runs to its end.
    async fn merge_and_deliver(&self) {
        self.merge().await;
        let jobs = &self.sessions.jobs;
        let started = async {
            loop {
                // Enabled before the look, so a start after it wakes this.
                let changed = jobs.changed();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self.heard().contains(&Heard::TurnRunning) {
                    return;
                }
                changed.await;
            }
        };
        timeout(TEST_BUDGET, started)
            .await
            .expect("the daemon starts a delivery turn");
        // The turn holds the runtime until its end is recorded.
        let ended = timeout(TEST_BUDGET, self.session.runtime.lock());
        drop(ended.await.expect("the delivery turn ends"));
    }

    /// What the session's feed told of its mark and its turns since the
    /// session began to watch.
    fn heard(&self) -> Vec<Heard> {
        self.session
            .feed
            .buffered_for_tests()
            .into_iter()
            .skip(self.told_before)
            .filter_map(|event| match event {
                wire::SessionEvent::State {
                    state: wire::JobState::Running,
                    ..
                } => Some(Heard::TurnRunning),
                wire::SessionEvent::MarkChanged { mark } => Some(Heard::Mark(mark)),
                wire::SessionEvent::TurnEnded {
                    outcome: wire::TurnOutcome::Answered { text, .. },
                    ..
                } => Some(Heard::Answered(text)),
                wire::SessionEvent::TurnEnded { outcome, .. } => Some(Heard::Unanswered(outcome)),
                _ => None,
            })
            .collect()
    }

    /// The session as a list reports it.
    fn listed(&self) -> SessionSummary {
        let root = self.root.path();
        let sessions =
            StoredSession::list_in_root_for_test(root, root, SessionListing::Everything).unwrap();
        let [session] = <[_; 1]>::try_from(sessions).expect("one session is saved");
        session
    }
}

// Covers: the turn that tells a session's agent its watched pull request
// merged leaves the session marked done, at a time not before the
// `updated_at` that turn's saves left, so a client reading
// `updated_at <= at` shows it finished once the turn is over, and tells
// the session's clients the mark before it tells them the turn ended. A
// daemon that marks the session only as it reads the merge leaves a mark
// the turn's saves outdate, and tells none here.
// Owner: daemon session marks.
#[tokio::test]
async fn the_turn_that_tells_of_a_merge_leaves_the_session_marked_done() {
    let fixture = Fixture::watching(Told::ByTheDaemon).await;

    fixture.merge_and_deliver().await;

    let listed = fixture.listed();
    let Some(SessionMark::Done { at }) = listed.mark else {
        panic!("the session is marked done, not {:?}", listed.mark);
    };
    assert_eq!(
        (at >= listed.updated_at, fixture.heard()),
        (
            true,
            vec![
                Heard::TurnRunning,
                Heard::Mark(Some(wire::SessionMark::Done { at })),
                Heard::Answered(NOTED.into()),
            ]
        )
    );
}

// Covers: a merge no turn tells the session's agent of, as while its
// client takes no daemon turns or after a daemon restart, still marks the
// session done as the daemon reads it, as of then rather than of the poll,
// and tells its clients. A daemon that marks only at the end of the
// telling turn leaves such a session unmarked. Owner: daemon session marks.
#[tokio::test]
async fn a_merge_no_turn_tells_of_marks_the_session_done_at_once() {
    let fixture = Fixture::watching(Told::AtTheNextPrompt).await;
    fixture.merge().await;

    // The daemon marks the merges it read before it stops.
    fixture.sessions.stopping.cancel();
    timeout(TEST_BUDGET, fixture.sessions.mark_merged_done())
        .await
        .expect("the daemon marks what it read, then stops");

    let listed = fixture.listed();
    let Some(SessionMark::Done { at }) = listed.mark else {
        panic!("the session is marked done, not {:?}", listed.mark);
    };
    assert_eq!(
        (at >= listed.updated_at, fixture.heard()),
        (
            true,
            vec![Heard::Mark(Some(wire::SessionMark::Done { at }))]
        )
    );
}

// Covers: a merge whose news waited for the person's next prompt, as after
// a daemon restart, marks nothing as that prompt's turn tells the agent:
// the person turned to the session again, so it is not marked done under
// them. Owner: daemon session marks.
#[tokio::test]
async fn a_prompt_that_tells_of_an_earlier_merge_marks_nothing() {
    let fixture = Fixture::watching(Told::AtTheNextPrompt).await;
    fixture.merge().await;
    let waited = fixture.inbox().has_news();

    fixture.prompt().await;

    assert_eq!(
        (
            (waited, fixture.inbox().has_news()),
            fixture.listed().mark,
            fixture.heard()
        ),
        (
            (true, false),
            None,
            vec![Heard::TurnRunning, Heard::Answered(NOTED.into())]
        )
    );
}
