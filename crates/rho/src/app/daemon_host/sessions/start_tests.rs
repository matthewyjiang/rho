use std::{sync::Arc, time::Duration};

use pretty_assertions::assert_eq;
use rho_protocol::{
    self as wire, ContentBlock, ErrorCode, IdempotencyKey, SessionPromptParams, TurnId,
};
use tempfile::TempDir;
use tokio::time::timeout;

use crate::app::daemon_host::{
    jobs::{JobRegistry, JobStore},
    sessions::{
        test_sessions::{load, Load},
        LoadedSession, Sessions,
    },
    wire::session_to_wire,
};

/// Bounds each wait on an explicit signal; never the way a test syncs.
const TEST_BUDGET: Duration = Duration::from_secs(5);

/// A prompt of `session` naming `key`: the turn it answers with, or the
/// code it is refused with.
async fn prompt(
    sessions: &Sessions,
    session: &LoadedSession,
    key: &str,
) -> Result<TurnId, Option<ErrorCode>> {
    let prompted = sessions.prompt(SessionPromptParams {
        session: session_to_wire(session.id.clone()),
        content: vec![ContentBlock::Text {
            text: "hello".into(),
        }],
        timeout_ms: None,
        environment: std::iter::empty::<(String, String)>().collect(),
        idempotency_key: Some(IdempotencyKey::new(key)),
    });
    timeout(TEST_BUDGET, prompted)
        .await
        .expect("the daemon answers the prompt")
        .map(|prompted| prompted.turn)
        .map_err(|error| error.error_code())
}

/// How many prompts `session`'s clients were shown.
fn prompts_shown(session: &LoadedSession) -> usize {
    session
        .feed
        .buffered_for_tests()
        .iter()
        .filter(|event| matches!(event, wire::SessionEvent::UserMessage { .. }))
        .count()
}

/// Cancels `session`'s turn and returns once it recorded its end.
async fn end_turn(session: &LoadedSession) {
    session.cancel_turn();
    let _ended = timeout(TEST_BUDGET, session.runtime.lock())
        .await
        .expect("the cancelled turn lets go of the runtime");
}

// Covers: a prompt sent again with its idempotency key starts no second
// turn and answers with the first one's turn, while that turn runs and
// after it ended; another key is another prompt, refused while the turn
// runs; and a turn that ended before it began ran nothing, so its key
// starts a turn (#1777).
// Owner: daemon session turn start.
#[tokio::test]
async fn a_prompt_sent_again_with_its_key_starts_no_second_turn() {
    let (root, home) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let sessions = Sessions::new(
        Arc::new(JobRegistry::open(JobStore::open(home.path()).unwrap())),
        home.path(),
    );
    let session = load(&sessions, &root, Load::Saved).await;

    // The provider never answers, so the turn runs until it is cancelled.
    let first = prompt(&sessions, &session, "k1").await;
    let again_while_running = prompt(&sessions, &session, "k1").await;
    let another_while_running = prompt(&sessions, &session, "k2").await;
    end_turn(&session).await;
    let again_once_ended = prompt(&sessions, &session, "k1").await;
    let shown = prompts_shown(&session);

    // A turn cancelled while it waits for the runtime ends before it began.
    let held = session.runtime.try_lock().unwrap();
    let unbegun = sessions.prompt(SessionPromptParams {
        session: session_to_wire(session.id.clone()),
        content: vec![ContentBlock::Text {
            text: "hello".into(),
        }],
        timeout_ms: None,
        environment: std::iter::empty::<(String, String)>().collect(),
        idempotency_key: Some(IdempotencyKey::new("k3")),
    });
    tokio::pin!(unbegun);
    assert!(futures_util::poll!(unbegun.as_mut()).is_pending());
    session.cancel_turn();
    drop(held);
    let unbegun = timeout(TEST_BUDGET, unbegun)
        .await
        .expect("the cancelled turn answers its prompt")
        .map_err(|error| error.error_code());
    drop(timeout(TEST_BUDGET, session.runtime.lock()).await.unwrap());
    let again_unbegun = prompt(&sessions, &session, "k3").await;
    let shown_after = prompts_shown(&session);
    end_turn(&session).await;

    let first = first.expect("the first prompt starts a turn");
    assert_eq!(
        (
            again_while_running,
            another_while_running,
            again_once_ended,
            shown,
            unbegun.map(|_| ()),
            again_unbegun.as_ref().map(|turn| *turn != first),
            shown_after,
        ),
        (
            Ok(first.clone()),
            Err(Some(ErrorCode::SessionBusy)),
            Ok(first.clone()),
            1,
            Err(Some(ErrorCode::TurnEnded)),
            Ok(true),
            2,
        )
    );
}
