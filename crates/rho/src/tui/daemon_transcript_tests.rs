use crate::tui::{
    daemon_events::{Rest, TurnState},
    tests::test_app,
    Entry,
};

// Covers: a reset replay during a running turn must not leave the turn's
// start pointing into the old transcript, which stamps the duration on an
// earlier turn's reply.
// Owner: daemon transcript (reset replay vs turn receipt)
#[tokio::test]
async fn reset_replay_during_a_turn_places_its_start_after_the_rebuilt_rows() {
    let mut app = test_app();
    app.insert_entry(&Entry::User("prompt".into()));
    app.show_daemon_turn(TurnState::Running).await;

    // The reset replay: the saved transcript, the running turn's prompt
    // last, then its running state.
    app.clear_daemon_transcript();
    app.insert_entry(&Entry::User("earlier".into()));
    app.insert_entry(&Entry::Assistant("earlier reply".into()));
    app.insert_entry(&Entry::User("prompt".into()));
    app.show_daemon_turn(TurnState::Running).await;
    // A turn that ends with no reply row of its own, as a tool-only one.
    app.show_daemon_turn(TurnState::Resting(Rest::Ready)).await;

    assert!(matches!(
        app.history.entries(),
        [
            Entry::User(_),
            Entry::Assistant(earlier),
            Entry::User(_),
            Entry::Assistant(receipt),
        ] if earlier.worked_for.is_none() && receipt.worked_for.is_some()
    ));
}
