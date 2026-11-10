use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use rho_protocol::{
    self as wire, ContentBlock, SessionChange, SessionConfigureParams, SessionPromptParams,
};
use tempfile::TempDir;
use tokio::time::timeout;

use crate::app::daemon_host::{
    jobs::{JobRegistry, JobState, JobStore},
    sessions::{
        stopping,
        test_sessions::{load, Load},
        Hold, LoadedSession, Sessions,
    },
    wire::session_to_wire,
};

/// Bounds each wait on an explicit signal; never the way a test syncs.
const TEST_BUDGET: Duration = Duration::from_secs(5);

/// A daemon's sessions with nothing loaded, recording jobs under `home`.
fn new_sessions(home: &TempDir) -> Sessions {
    Sessions::new(
        Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
        home.path(),
    )
}

/// A change of `session` to plan mode, away from the config's bypass.
fn plan_mode(session: &LoadedSession) -> SessionConfigureParams {
    SessionConfigureParams {
        session: session_to_wire(session.id.clone()),
        change: SessionChange::PermissionMode {
            mode: wire::PermissionMode::Plan,
        },
    }
}

/// What clients saw, in the order they saw it.
#[derive(Debug, PartialEq)]
enum Seen {
    Options(wire::PermissionMode),
    Prompt,
}

// Covers: a change sent while the session's runtime is held, as by a turn
// that recorded its end but has not let go of it yet, queues and counts in
// the session's hold; a prompt sent meanwhile starts its turn rather than
// being refused busy, and that turn waits behind the change, so clients see
// the options change before the prompt (#1685).
// Owner: daemon session settings.
#[tokio::test]
async fn a_prompt_sent_while_a_change_waits_runs_after_it() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let sessions = new_sessions(&home);
    let session = load(&sessions, &root, Load::Saved).await;
    let ended_turn = session.runtime.try_lock().unwrap();

    let configure = sessions.configure(plan_mode(&session));
    tokio::pin!(configure);
    // Nothing in a configure of a loaded session waits before the runtime.
    assert!(futures_util::poll!(configure.as_mut()).is_pending());
    let queued = session.turn().hold;
    let prompt = sessions.prompt(SessionPromptParams {
        session: session_to_wire(session.id.clone()),
        content: vec![ContentBlock::Text {
            text: "hello".into(),
        }],
        timeout_ms: None,
        environment: std::iter::empty::<(String, String)>().collect(),
        idempotency_key: None,
    });
    tokio::pin!(prompt);
    // Pending until its turn, behind the change, saves the prompt.
    assert!(futures_util::poll!(prompt.as_mut()).is_pending());
    let job = sessions.jobs.get(&session.job).unwrap();
    let (started, turn) = (job.state, job.turn.map(|turn| turn.id));
    drop(ended_turn);
    let changed = timeout(TEST_BUDGET, configure)
        .await
        .expect("the change applies once the runtime is free")
        .map(|changed| changed.options.permission_mode.current);
    let prompted = timeout(TEST_BUDGET, prompt)
        .await
        .expect("the turn saves its prompt once the change applied")
        .map(|prompted| Some(prompted.turn));

    let seen: Vec<_> = session
        .feed
        .buffered_for_tests()
        .into_iter()
        .filter_map(|event| match event {
            wire::SessionEvent::OptionsChanged { options } => {
                Some(Seen::Options(options.permission_mode.current))
            }
            wire::SessionEvent::UserMessage { .. } => Some(Seen::Prompt),
            _ => None,
        })
        .collect();
    // The turn's model call waits for a cancel: end it before the test does.
    session.cancel_turn();
    let _ended = timeout(TEST_BUDGET, session.runtime.lock())
        .await
        .expect("the cancelled turn lets go of the runtime");
    assert_eq!(
        (queued, started, changed, prompted, seen),
        (
            Some(Hold::Configuring(NonZeroUsize::MIN)),
            JobState::Running,
            Ok(wire::PermissionMode::Plan),
            // The prompt answers with the turn its job recorded.
            Ok(turn),
            vec![Seen::Options(wire::PermissionMode::Plan), Seen::Prompt],
        )
    );
}

// Covers: the daemon's stop answers a change still queued on the runtime
// `DaemonStopping` and frees its part of the session's hold, rather than
// keeping the request, and so the daemon, waiting on a turn that may
// outlive the stop's drain.
// Owner: daemon session settings.
#[tokio::test]
async fn the_stop_answers_a_queued_change() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let sessions = new_sessions(&home);
    let session = load(&sessions, &root, Load::Saved).await;
    let _turn = session.runtime.try_lock().unwrap();
    let configure = sessions.configure(plan_mode(&session));
    tokio::pin!(configure);
    assert!(futures_util::poll!(configure.as_mut()).is_pending());

    sessions.begin_stop(rho_protocol::DetachReason::DaemonStopping);
    let answered = timeout(TEST_BUDGET, configure)
        .await
        .expect("the stop answers the change")
        .map(|_| ());

    assert_eq!((answered, session.turn().hold), (Err(stopping()), None));
}

// Covers: context a client adds between turns reaches the model's history
// as sent and joins the transcript a later attach replays as the row that
// displays it, though no turn ran to save it; empty context is refused.
// Owner: daemon session settings.
#[tokio::test]
async fn added_context_reaches_the_model_and_a_later_replay() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let sessions = new_sessions(&home);
    let session = load(&sessions, &root, Load::Saved).await;
    let context = |text: &str| SessionConfigureParams {
        session: session_to_wire(session.id.clone()),
        change: SessionChange::Context {
            text: text.into(),
            display: "remembered in AGENTS.md".into(),
        },
    };

    let refused = sessions
        .configure(context("  "))
        .await
        .map(|_| ())
        .map_err(|error| error.code);
    let added = sessions
        .configure(context("Apply it: be brief"))
        .await
        .map(|_| ());

    let replayed: Vec<_> = session
        .feed
        .reset_replay_for_tests()
        .into_iter()
        .filter_map(|event| match event {
            wire::SessionEvent::UserMessage { content, .. } => Some(content),
            _ => None,
        })
        .collect();
    let read_by_model = session
        .runtime
        .try_lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .history_for_tests()
        .contains(&rho_sdk::model::Message::user_text("Apply it: be brief"));
    assert_eq!(
        (refused, added, replayed, read_by_model),
        (
            Err(wire::ErrorCode::InvalidParams.code()),
            Ok(()),
            vec![vec![ContentBlock::Text {
                text: "remembered in AGENTS.md".into(),
            }]],
            true,
        )
    );
}
