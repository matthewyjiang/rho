use std::{sync::Arc, time::SystemTime};

use pretty_assertions::assert_eq;
use rho_protocol::{
    self as wire, ContentBlock, ErrorCode, IdempotencyKey, InputAnswer, InputRequestId,
    SessionPromptParams, TurnId,
};
use tempfile::TempDir;
use tokio::{sync::mpsc, time::timeout};

use super::super::{
    inputs::Asker,
    test_events::{approval, elicitation, plan, question},
    test_sessions::{load, prompts_shown, seen, until, Load, Seen, TEST_BUDGET},
    LoadedSession, Sessions,
};
use crate::app::{
    daemon_host::{
        jobs::{JobRegistry, JobStore},
        wire::session_to_wire,
    },
    peer_sessions::{PeerEvent, PeerInbox, PeerNotice, PeerTurnEnd},
    pull_requests::ScriptedForge,
    session_runtime::SessionEvent,
};

fn prompt_params(session: &LoadedSession, key: Option<IdempotencyKey>) -> SessionPromptParams {
    SessionPromptParams {
        author: wire::PromptAuthor::Person,
        session: session_to_wire(session.id.clone()),
        content: vec![ContentBlock::Text {
            text: "hello".into(),
        }],
        timeout_ms: None,
        environment: std::iter::empty::<(String, String)>().collect(),
        idempotency_key: key,
        grant: None,
    }
}

/// Prompts `session` under `key`; returns once its turn began. The provider
/// never answers, so the turn runs until it is cancelled.
async fn prompt(
    sessions: &Sessions,
    session: &LoadedSession,
    key: Option<IdempotencyKey>,
) -> Result<TurnId, Option<ErrorCode>> {
    timeout(TEST_BUDGET, sessions.prompt(prompt_params(session, key)))
        .await
        .expect("the daemon answers the prompt")
        .map(|started| started.response.turn)
        .map_err(|error| error.error_code())
}

/// Prompts `session` as the `sessions` tool of the session whose inbox is
/// `caller` does: registered first, the registration gone once the daemon
/// answered.
async fn prompt_from(
    sessions: &Sessions,
    caller: &PeerInbox,
    session: &LoadedSession,
) -> Result<TurnId, Option<ErrorCode>> {
    let registration = caller.register(session.id.as_str());
    prompt(sessions, session, Some(registration.key())).await
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

fn ended(session: &LoadedSession) -> PeerNotice {
    PeerNotice {
        session: session.id.to_string(),
        title: None,
        event: PeerEvent::TurnEnded(PeerTurnEnd::Cancelled),
    }
}

fn asking(session: &LoadedSession, request: u64) -> PeerNotice {
    PeerNotice {
        session: session.id.to_string(),
        title: None,
        event: PeerEvent::WaitingForInput {
            request: InputRequestId(request),
            ask: "ask".into(),
        },
    }
}

/// A daemon with a starter session whose client asked for daemon turns
/// (`idle_wake: turn`), so it has an inbox for the turns it starts.
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
        let starter = load_starter(&sessions, &root).await;
        Self {
            root,
            _home: home,
            sessions,
            starter,
        }
    }

    /// Another session, as session control's `new` opens one.
    async fn session(&self) -> Arc<LoadedSession> {
        load(&self.sessions, &self.root, Load::Saved).await
    }

    fn inbox(&self) -> &PeerInbox {
        inbox(&self.starter)
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

/// A session that takes daemon turns, as the daemon adds it. It has history,
/// as a session that started a turn elsewhere does, so the daemon's turn has
/// something to continue.
async fn load_starter(sessions: &Sessions, root: &TempDir) -> Arc<LoadedSession> {
    let starter = load(sessions, root, Load::Waking).await;
    sessions
        .configure(wire::SessionConfigureParams {
            session: session_to_wire(starter.id.clone()),
            change: wire::SessionChange::Context {
                text: "earlier work".into(),
                display: "earlier work".into(),
            },
        })
        .await
        .expect("the starter takes context");
    starter.report_activity();
    starter
}

fn inbox(session: &LoadedSession) -> &PeerInbox {
    session
        .peer_wake
        .inbox()
        .expect("a session that takes daemon turns has an inbox")
}

// Covers: a turn the starter's call started keeps the starter active while
// it runs; once every such turn ended, the idle starter, which no client
// follows, is woken by exactly one turn that shows each notice, oldest
// first, and leaves nothing outstanding. A miss leaves the starter polling
// `sessions wait`, floods it with a turn per notice, or loops delivery
// turns.
// Owner: daemon wake for started sessions.
#[tokio::test]
async fn one_turn_wakes_an_idle_starter_for_the_turns_it_started() {
    use wire::SessionActivity::{Active, Settled};
    for count in [1, 3] {
        let fixture = Fixture::watched().await;
        let mut children = Vec::new();
        for _ in 0..count {
            let child = fixture.session().await;
            prompt_from(&fixture.sessions, fixture.inbox(), &child)
                .await
                .expect("the prompt starts a turn");
            children.push(child);
        }
        let owed_while_running = fixture.inbox().outstanding();

        // Held, so a delivery turn the first end starts takes nothing until
        // every turn ended.
        let held = fixture.starter.runtime.lock().await;
        for child in &children {
            cancel(&fixture.sessions, child).await;
        }
        drop(held);
        fixture.shown_then_cancelled(count).await;

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
                children
                    .iter()
                    .map(|child| Some(child.id.to_string()))
                    .collect::<Vec<_>>(),
                false,
            ),
            "{count} started"
        );
    }
}

/// Which key a case's prompt carries.
#[derive(Clone, Copy, Debug)]
enum Key {
    /// One the starter registered.
    Registered,
    /// None, as a person's client or a caller the daemon tells nothing
    /// sends.
    Absent,
    /// The client's own, which nobody registered.
    Unregistered,
    /// One the starter registered for another session.
    ForAnother,
}

// Covers: only the turn a prompt the starter registered starts tells the
// starter; a turn a person, or a caller with no inbox, starts tells nobody,
// and so does one whose prompt names a key nobody registered, or one the
// starter registered for another session. A miss wakes an agent for its
// user's own work.
// Owner: daemon wake for started sessions.
#[tokio::test]
async fn only_the_turn_a_registered_prompt_starts_tells_the_starter() {
    let cases = [
        (Key::Registered, true),
        (Key::Absent, false),
        (Key::Unregistered, false),
        (Key::ForAnother, false),
    ];
    for (key, tells) in cases {
        let fixture = Fixture::unwatched().await;
        let child = fixture.session().await;
        let started = match key {
            Key::Registered => prompt_from(&fixture.sessions, fixture.inbox(), &child).await,
            Key::Absent => prompt(&fixture.sessions, &child, /*key*/ None).await,
            Key::Unregistered => {
                let key = IdempotencyKey::new("the client's own");
                prompt(&fixture.sessions, &child, Some(key)).await
            }
            Key::ForAnother => {
                let elsewhere = fixture.session().await;
                let registration = fixture.inbox().register(elsewhere.id.as_str());
                prompt(&fixture.sessions, &child, Some(registration.key())).await
            }
        };
        started.expect("the prompt starts a turn");
        let owed_while_running = fixture.inbox().outstanding();
        cancel(&fixture.sessions, &child).await;
        let expected = if tells {
            vec![ended(&child)]
        } else {
            Vec::new()
        };

        assert_eq!(
            (owed_while_running, fixture.inbox().take().notices),
            (tells, expected),
            "{key:?}"
        );
    }
}

// Covers: of two callers that registered a prompt of one session, a turn
// tells only the one whose prompt started it; a second caller whose prompt
// the busy session refuses owes nothing, and a turn it starts once that one
// ended tells it alone. A miss tells a caller of a turn it never started,
// or keeps the refused caller loaded forever.
// Owner: daemon wake for started sessions.
#[tokio::test]
async fn a_turn_tells_only_the_caller_whose_prompt_started_it() {
    let fixture = Fixture::unwatched().await;
    let other = load_starter(&fixture.sessions, &fixture.root).await;
    let child = fixture.session().await;
    let (first, second) = (fixture.inbox(), inbox(&other));

    // Each turn starts while the other caller holds a registration of the
    // child too, so a claim on the session alone tells the wrong caller of
    // one of the two turns, whichever inbox the daemon looks in first.
    let second_registered = second.register(child.id.as_str());
    prompt_from(&fixture.sessions, first, &child)
        .await
        .expect("the prompt starts a turn");
    let refused = prompt(&fixture.sessions, &child, Some(second_registered.key()))
        .await
        .map(|_| ());
    let second_owed_once_refused = second.outstanding();
    cancel(&fixture.sessions, &child).await;
    let told_of_the_first = (first.take().notices, second.take().notices);
    drop(second_registered);
    let _first_registered = first.register(child.id.as_str());
    prompt_from(&fixture.sessions, second, &child)
        .await
        .expect("the prompt starts a turn");
    cancel(&fixture.sessions, &child).await;
    let told_of_the_second = (first.take().notices, second.take().notices);

    assert_eq!(
        (
            refused,
            second_owed_once_refused,
            told_of_the_first,
            told_of_the_second
        ),
        (
            Err(Some(ErrorCode::SessionBusy)),
            false,
            (vec![ended(&child)], Vec::new()),
            (Vec::new(), vec![ended(&child)]),
        )
    );
}

// Covers: a started turn tells its starter of each question once, as it
// opens, naming the request and what it asks, though it closes before a
// boundary takes the notice, and its close tells nothing; an approval, a
// plan approval or a question for the user alone, as an MCP server's
// elicitation is, tells nothing, since only a person answers it; the turn's
// end follows. A miss leaves the starter waiting on a session that waits
// on a person, or tells it of one question twice.
// Owner: daemon wake for started sessions.
#[tokio::test]
async fn a_started_turn_tells_its_starter_of_each_question_once() {
    let fixture = Fixture::unwatched().await;
    let child = fixture.session().await;
    prompt_from(&fixture.sessions, fixture.inbox(), &child)
        .await
        .expect("the prompt starts a turn");
    let (control, _answers) = mpsc::unbounded_channel();
    let ask = |event: &SessionEvent| {
        child.change_inputs(|inputs| inputs.note(Asker::Turn, &control, event, SystemTime::now()));
    };
    let answer = |request| {
        child
            .change_inputs(|inputs| inputs.answer(InputRequestId(request), InputAnswer::Decline))
            .unwrap();
    };
    let taken = || fixture.inbox().take().notices;

    ask(&approval(0));
    let approval_opened = taken();
    ask(&plan(0));
    let plan_opened = taken();
    ask(&question("pick"));
    answer(2);
    let closed_before_taken = taken();
    ask(&question("again"));
    let opened = taken();
    answer(3);
    let closed_once_taken = taken();
    ask(&elicitation("server"));
    let user_only_opened = taken();
    cancel(&fixture.sessions, &child).await;
    let turn_ended = taken();

    assert_eq!(
        [
            approval_opened,
            plan_opened,
            closed_before_taken,
            opened,
            closed_once_taken,
            user_only_opened,
            turn_ended
        ],
        [
            Vec::new(),
            Vec::new(),
            vec![asking(&child, 2)],
            vec![asking(&child, 3)],
            Vec::new(),
            Vec::new(),
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
    let child = fixture.session().await;
    prompt_from(&fixture.sessions, fixture.inbox(), &child)
        .await
        .expect("the prompt starts a turn");

    // The starter's turn starts, and waits for the runtime held here
    // until the notice waits.
    let held = fixture.starter.runtime.lock().await;
    let prompted = fixture
        .sessions
        .prompt(prompt_params(&fixture.starter, /*key*/ None));
    tokio::pin!(prompted);
    let starter_running = || {
        fixture
            .sessions
            .jobs
            .get(&fixture.starter.job)
            .is_some_and(|job| job.state.in_progress())
    };
    tokio::select! {
        _ = prompted.as_mut() => panic!("the prompt answers before its turn could begin"),
        () = until(starter_running, "the starter's turn starts") => {}
    }
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
            1,
            false,
        )
    );
}

// Covers: a started turn that ends after the starter's delivery turn took
// its notices, and before that turn ended, wakes the starter again once it
// did. A miss leaves a starter that runs several sessions untold of the
// later ones until something else starts a turn.
// Owner: daemon wake for started sessions.
#[tokio::test]
async fn a_turn_that_ends_during_the_delivery_turn_wakes_the_starter_again() {
    let fixture = Fixture::watched().await;
    let (first, second) = (fixture.session().await, fixture.session().await);
    for child in [&first, &second] {
        prompt_from(&fixture.sessions, fixture.inbox(), child)
            .await
            .expect("the prompt starts a turn");
    }

    cancel(&fixture.sessions, &first).await;
    until(
        || cards(&fixture.starter).len() == 1,
        "the starter shows the first notice",
    )
    .await;
    // The delivery turn is past the boundary that took the first notice.
    cancel(&fixture.sessions, &second).await;
    let left_behind = fixture.inbox().has_notices();
    // Not awaited to its end: the next delivery turn may be under way by
    // the time this task looks.
    fixture.starter.cancel_turn();
    fixture.shown_then_cancelled(2).await;

    assert_eq!(
        (
            left_behind,
            cards(&fixture.starter),
            fixture.inbox().outstanding(),
        ),
        (
            true,
            vec![Some(first.id.to_string()), Some(second.id.to_string())],
            false,
        )
    );
}
