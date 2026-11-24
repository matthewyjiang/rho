use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use pretty_assertions::assert_eq;
use rho_protocol::{
    self as wire, ContentBlock, InputAnswer, InputRequestId, SessionPromptParams, TurnId,
};
use tempfile::TempDir;
use tokio::{sync::mpsc, time::timeout};

use super::super::{
    inputs::Asker,
    test_events::{approval, question},
    test_sessions::{load, load_started_by, Load},
    LoadedSession, Sessions,
};
use crate::{
    app::{
        daemon_host::{
            jobs::{JobRegistry, JobStore},
            wire::session_to_wire,
        },
        peer_sessions::{PeerEvent, PeerInbox, PeerNotice, PeerTurnEnd},
        pull_requests::ScriptedForge,
        session_runtime::SessionEvent,
    },
    caller_context::CallerContext,
};

/// Bounds each wait on the daemon's own tasks; never the way a test syncs.
const TEST_BUDGET: Duration = Duration::from_secs(10);

/// Runs the daemon's tasks until `done` holds; [`TEST_BUDGET`] bounds a
/// hang.
async fn until(done: impl Fn() -> bool, what: &str) {
    let wait = async {
        while !done() {
            tokio::task::yield_now().await;
        }
    };
    timeout(TEST_BUDGET, wait)
        .await
        .unwrap_or_else(|_| panic!("{what}"));
}

/// Who sends a prompt.
#[derive(Clone, Copy, Debug)]
enum By {
    /// Session control, which names the prompt's grant.
    SessionControl,
    /// A person's client, which leaves it to its environment.
    Person,
}

fn prompt_params(session: &LoadedSession, by: By) -> SessionPromptParams {
    SessionPromptParams {
        author: wire::PromptAuthor::Person,
        session: session_to_wire(session.id.clone()),
        content: vec![ContentBlock::Text {
            text: "hello".into(),
        }],
        timeout_ms: None,
        environment: std::iter::empty::<(String, String)>().collect(),
        idempotency_key: None,
        grant: match by {
            By::SessionControl => Some(CallerContext::ROOT.into()),
            By::Person => None,
        },
    }
}

/// Prompts `session` as `by` does; returns once its turn began. The
/// provider never answers, so the turn runs until it is cancelled.
async fn prompt(sessions: &Sessions, session: &LoadedSession, by: By) -> TurnId {
    timeout(TEST_BUDGET, sessions.prompt(prompt_params(session, by)))
        .await
        .expect("the daemon answers the prompt")
        .expect("the prompt starts a turn")
        .turn
}

/// Cancels `session`'s turn and returns once its job recorded the end.
async fn cancel(sessions: &Sessions, session: &LoadedSession) {
    session.cancel_turn();
    let ended = || {
        sessions
            .jobs
            .get(&session.job)
            .is_some_and(|job| !job.state.in_progress())
    };
    until(ended, "the cancelled turn ends").await;
}

/// What a session's clients heard of its activity and its turns, each
/// change once.
#[derive(Debug, PartialEq)]
enum Seen {
    Activity(wire::SessionActivity),
    TurnRunning,
    TurnEnded,
}

fn seen(session: &LoadedSession) -> Vec<Seen> {
    let mut seen: Vec<Seen> = session
        .feed
        .buffered_for_tests()
        .into_iter()
        .filter_map(|event| match event {
            wire::SessionEvent::Activity { state } => Some(Seen::Activity(state)),
            wire::SessionEvent::State { state, .. } if state.in_progress() => {
                Some(Seen::TurnRunning)
            }
            wire::SessionEvent::State { .. } => Some(Seen::TurnEnded),
            _ => None,
        })
        .collect();
    seen.dedup();
    seen
}

/// The sessions the notification cards a session's clients were shown
/// refer to, in order.
fn cards(session: &LoadedSession) -> Vec<Option<String>> {
    session
        .feed
        .buffered_for_tests()
        .into_iter()
        .filter_map(|event| match event {
            wire::SessionEvent::Card {
                display: Some(card),
                ..
            } => Some(card.reference),
            _ => None,
        })
        .collect()
}

/// How many prompts a session's clients were shown.
fn prompts_shown(session: &LoadedSession) -> usize {
    session
        .feed
        .buffered_for_tests()
        .iter()
        .filter(|event| matches!(event, wire::SessionEvent::UserMessage { .. }))
        .count()
}

fn ended(session: &LoadedSession) -> PeerNotice {
    PeerNotice {
        session: session.id.to_string(),
        title: None,
        event: PeerEvent::TurnEnded(PeerTurnEnd::Cancelled),
    }
}

fn waiting(session: &LoadedSession, request: u64, title: &str) -> PeerNotice {
    PeerNotice {
        session: session.id.to_string(),
        title: None,
        event: PeerEvent::WaitingForInput {
            request: InputRequestId(request),
            title: title.into(),
        },
    }
}

/// A daemon with a starter session whose client asked for daemon turns
/// (`idle_wake: turn`), so it has an inbox for the sessions it starts.
struct Fixture {
    root: TempDir,
    _home: TempDir,
    sessions: Arc<Sessions>,
    starter: Arc<LoadedSession>,
}

impl Fixture {
    /// The starter, its idle watcher running, as the daemon adds it.
    async fn watched() -> Self {
        let fixture = Self::unwatched().await;
        fixture.sessions.watch_idle_results(&fixture.starter);
        fixture
    }

    /// The starter with no idle watcher, so nothing takes its notices but
    /// the test.
    async fn unwatched() -> Self {
        let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        let sessions = Sessions::new(
            Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
            home.path(),
            ScriptedForge::new(),
        );
        let starter = load(&sessions, &root, Load::Waking).await;
        // As the session is added.
        starter.report_activity();
        Self {
            root,
            _home: home,
            sessions,
            starter,
        }
    }

    /// A session the starter started, as session control's `new` opens it.
    async fn started(&self) -> Arc<LoadedSession> {
        load_started_by(&self.sessions, &self.root, &self.starter.job).await
    }

    fn inbox(&self) -> &PeerInbox {
        self.starter
            .peers
            .as_ref()
            .expect("a session that takes daemon turns has an inbox")
    }

    /// Waits until the starter's clients were shown `count` notification
    /// cards, then cancels the starter's turn that showed them.
    async fn shown_then_cancelled(&self, count: usize) {
        until(
            || cards(&self.starter).len() == count,
            "the starter shows the notices",
        )
        .await;
        cancel(&self.sessions, &self.starter).await;
    }
}

// Covers: a turn the starter began through session control in a session it
// started keeps the starter active while it runs, and once it ends, the
// idle starter, which no client follows, is woken by exactly one turn that
// shows the notice and leaves nothing to deliver. A miss leaves the starter
// polling `sessions wait`, or loops delivery turns.
// Owner: daemon wake for started sessions.
#[tokio::test]
async fn an_idle_starter_is_woken_once_when_a_turn_it_began_ends() {
    use wire::SessionActivity::{Active, Settled};
    let fixture = Fixture::watched().await;
    let child = fixture.started().await;

    prompt(&fixture.sessions, &child, By::SessionControl).await;
    let owed_while_running = fixture.inbox().outstanding();
    cancel(&fixture.sessions, &child).await;
    fixture.shown_then_cancelled(1).await;

    assert_eq!(
        (
            owed_while_running,
            seen(&fixture.starter),
            cards(&fixture.starter),
            fixture.inbox().outstanding(),
        ),
        (
            true,
            vec![
                Seen::Activity(Settled),
                Seen::Activity(Active),
                Seen::TurnRunning,
                Seen::TurnEnded,
                Seen::Activity(Settled),
            ],
            vec![Some(child.id.to_string())],
            false,
        )
    );
}

// Covers: three started sessions whose turns end while the starter is idle
// are told in one delivery turn, oldest end first, and no second turn
// follows. A turn per notice floods the starter.
// Owner: daemon wake for started sessions.
#[tokio::test]
async fn one_turn_takes_the_notices_of_three_started_sessions() {
    use wire::SessionActivity::{Active, Settled};
    let fixture = Fixture::watched().await;
    let mut children = Vec::new();
    for _ in 0..3 {
        let child = fixture.started().await;
        prompt(&fixture.sessions, &child, By::SessionControl).await;
        children.push(child);
    }

    // Held, so a delivery turn the first end starts takes nothing until
    // all three ended.
    let held = fixture.starter.runtime.lock().await;
    for child in &children {
        cancel(&fixture.sessions, child).await;
    }
    drop(held);
    fixture.shown_then_cancelled(3).await;

    assert_eq!(
        (seen(&fixture.starter), cards(&fixture.starter)),
        (
            vec![
                Seen::Activity(Settled),
                Seen::Activity(Active),
                Seen::TurnRunning,
                Seen::TurnEnded,
                Seen::Activity(Settled),
            ],
            children
                .iter()
                .map(|child| Some(child.id.to_string()))
                .collect::<Vec<_>>(),
        )
    );
}

/// Which session a case prompts.
#[derive(Clone, Copy, Debug)]
enum Prompted {
    /// One the starter started.
    Started,
    /// One no session started.
    Unrelated,
}

// Covers: only a turn begun through session control in a session the
// starter started tells the starter; one a person begins in that session,
// once it is handed to them, does not, nor does a session-control turn in
// a session nobody started. A miss wakes an agent for its user's own work.
// Owner: daemon wake for started sessions.
#[tokio::test]
async fn only_a_session_control_turn_in_a_started_session_tells_the_starter() {
    let cases = [
        (Prompted::Started, By::SessionControl, true),
        (Prompted::Started, By::Person, false),
        (Prompted::Unrelated, By::SessionControl, false),
    ];
    for (prompted, by, tells) in cases {
        let fixture = Fixture::unwatched().await;
        let session = match prompted {
            Prompted::Started => fixture.started().await,
            Prompted::Unrelated => load(&fixture.sessions, &fixture.root, Load::Saved).await,
        };
        prompt(&fixture.sessions, &session, by).await;
        let owed_while_running = fixture.inbox().outstanding();
        cancel(&fixture.sessions, &session).await;
        let expected = if tells {
            vec![ended(&session)]
        } else {
            Vec::new()
        };

        assert_eq!(
            (
                owed_while_running,
                fixture.inbox().take().notices_for_tests()
            ),
            (tells, expected),
            "{prompted:?} prompted by {by:?}"
        );
    }
}

// Covers: a started session's turn that stops for a person tells the
// starter once per input request, naming it and what it asks; an answer
// tells nothing; the turn's end follows. A miss leaves the starter waiting
// on a session that waits on a person, or tells it of one request twice.
// Owner: daemon wake for started sessions.
#[tokio::test]
async fn a_started_turn_tells_its_starter_once_per_input_request() {
    let fixture = Fixture::unwatched().await;
    let child = fixture.started().await;
    prompt(&fixture.sessions, &child, By::SessionControl).await;
    let (control, _answers) = mpsc::unbounded_channel();
    let ask = |event: &SessionEvent| {
        child.change_inputs(|inputs| inputs.note(Asker::Turn, &control, event, SystemTime::now()));
    };
    let taken = || fixture.inbox().take().notices_for_tests();

    ask(&approval(0));
    let approval_opened = taken();
    child
        .change_inputs(|inputs| inputs.answer(InputRequestId(0), InputAnswer::AllowOnce))
        .unwrap();
    let approval_answered = taken();
    ask(&question("pick"));
    let question_opened = taken();
    cancel(&fixture.sessions, &child).await;
    let turn_ended = taken();

    assert_eq!(
        [
            approval_opened,
            approval_answered,
            question_opened,
            turn_ended
        ],
        [
            vec![waiting(&child, 0, "tool approval")],
            Vec::new(),
            vec![waiting(&child, 1, "ask")],
            vec![ended(&child)],
        ]
    );
}

// Covers: a notice that arrives while the starter's own turn is under way
// is taken at that turn's boundary, and the daemon starts no turn for it
// after. A miss tells the starter twice, or starts a turn it never needed.
// Owner: daemon wake for started sessions.
#[tokio::test]
async fn a_busy_starter_takes_the_notice_at_its_boundary_and_no_turn_follows() {
    use wire::SessionActivity::{Active, Settled};
    let fixture = Fixture::watched().await;
    let child = fixture.started().await;
    prompt(&fixture.sessions, &child, By::SessionControl).await;

    // The starter's turn starts, and waits for the runtime held here
    // until the notice waits.
    let held = fixture.starter.runtime.lock().await;
    let prompted = fixture
        .sessions
        .prompt(prompt_params(&fixture.starter, By::Person));
    tokio::pin!(prompted);
    assert!(futures_util::poll!(prompted.as_mut()).is_pending());
    cancel(&fixture.sessions, &child).await;
    let waited_for_the_boundary = fixture.inbox().has_notices();
    drop(held);
    timeout(TEST_BUDGET, prompted)
        .await
        .expect("the daemon answers the prompt")
        .expect("the prompt starts a turn");
    fixture.shown_then_cancelled(1).await;

    assert_eq!(
        (
            waited_for_the_boundary,
            seen(&fixture.starter),
            cards(&fixture.starter),
            prompts_shown(&fixture.starter),
        ),
        (
            true,
            vec![
                Seen::Activity(Settled),
                Seen::Activity(Active),
                Seen::TurnRunning,
                Seen::TurnEnded,
                Seen::Activity(Settled),
            ],
            vec![Some(child.id.to_string())],
            1,
        )
    );
}
