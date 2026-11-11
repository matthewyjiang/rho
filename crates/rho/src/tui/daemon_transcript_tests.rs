use pretty_assertions::assert_eq;

use crate::tui::{
    daemon_events::{Rest, TurnState},
    tests::test_app,
    Entry, StreamKind,
};

// Covers: a replayed chunk that continues the message a running turn
// streams, as a reattach's tail brings, leaves the stream knowing what it
// said: a code block it opened stays open for the live text that follows,
// and the turn's answer still completes the message rather than
// mismatching it (#1818).
// Owner: daemon transcript (tail replay into a live stream)
#[test]
fn a_replayed_continuation_keeps_the_stream_in_step_with_the_message() {
    let mut app = test_app();
    app.streams.assistant_stream.push_delta("Here:\n");
    app.finish_streams();

    app.continue_stream_with_replayed(StreamKind::Assistant, "```rust\nlet a = 1;\n");

    let shown = match app.history.entries() {
        [Entry::Assistant(reply)] => reply.text.clone(),
        _ => String::new(),
    };
    let said = "Here:\n```rust\nlet a = 1;\n";
    assert_eq!(
        (
            app.streams.code_fence(StreamKind::Assistant).is_open(),
            app.streams.assistant_stream.emitted_text(),
            shown.as_str(),
        ),
        (true, said, said)
    );
}

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
