use super::{HistorySink, SessionHistorySink};
use crate::{model::Message, session::Session};

#[test]
fn persists_queued_messages_in_order_before_drop_returns() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session = Session::create_in_root(root.path(), cwd.path()).unwrap();
    let id = session.id().to_owned();
    let mut sink = SessionHistorySink::new(session);

    sink.append_message(&Message::user_text("first")).unwrap();
    sink.append_message(&Message::assistant_text("second"))
        .unwrap();
    drop(sink);

    let histories = Session::open_by_id_with_histories_in_root(root.path(), cwd.path(), &id)
        .unwrap()
        .1;
    assert_eq!(histories.display.len(), 2);
    assert!(matches!(
        &histories.display[0],
        Message::User(blocks) if matches!(blocks.as_slice(), [crate::model::ContentBlock::Text(text)] if text == "first")
    ));
    assert!(matches!(
        &histories.display[1],
        Message::Assistant(blocks) if matches!(blocks.as_slice(), [crate::model::ContentBlock::Text(text)] if text == "second")
    ));
}
