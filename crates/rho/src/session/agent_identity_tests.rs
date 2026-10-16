use std::fs;

use pretty_assertions::assert_eq;
use rho_providers::model::Message;

use super::super::{tree::SessionTree, Session};

// Covers: fresh delegated rebinding must survive resume and later appends, but
// must never rewrite identity once a transcript contains history.
// Owner: session agent identity.
#[test]
fn fresh_agent_identity_rewrite_preserves_append_and_resume() {
    let root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let session =
        Session::create_in_root_with_agent(root.path(), cwd.path(), "root", "old").unwrap();
    // Prime the tree cache before replacing the header.
    session.cache_loaded_tree(SessionTree::load(&session.path).unwrap());
    assert_eq!(session.delegated_agent_id().unwrap(), None);
    session
        .rebind_fresh_delegated_agent("restricted-child", "new-fingerprint")
        .unwrap();
    let expected_identity = Some(("restricted-child".into(), "new-fingerprint".into()));
    assert_eq!(session.stored_agent_identity().unwrap(), expected_identity);
    assert_eq!(
        session.delegated_agent_id().unwrap(),
        Some("restricted-child".into())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&session.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let message = Message::user_text("retained task");
    session.append_message(&message).unwrap();
    let before = fs::read(&session.path).unwrap();
    assert!(session.rebind_fresh_delegated_agent("root", "old").is_err());
    assert_eq!(fs::read(&session.path).unwrap(), before);
    let (reopened, histories) =
        Session::open_by_id_with_histories_in_root(root.path(), cwd.path(), session.id()).unwrap();
    assert_eq!(reopened.stored_agent_identity().unwrap(), expected_identity);
    assert_eq!(
        reopened.delegated_agent_id().unwrap(),
        Some("restricted-child".into())
    );
    assert_eq!(
        (histories.model, histories.display),
        (vec![message.clone()], vec![message])
    );
}
