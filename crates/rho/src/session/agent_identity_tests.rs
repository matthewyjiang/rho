use std::fs;

use pretty_assertions::assert_eq;
use rho_providers::model::{Message, ModelIdentity};

use super::super::{tree::SessionTree, Session, SESSION_VERSION};

// Covers: fresh delegated rebinding commits its identity and initial state
// together for current/legacy headers, survives resume and later appends, and
// cannot rewrite history.
// Owner: session agent identity.
#[test]
fn fresh_agent_identity_rewrite_preserves_append_and_resume() {
    for version in 1..=SESSION_VERSION {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let session =
            Session::create_in_root_with_agent(root.path(), cwd.path(), "root", "old").unwrap();
        // A legacy session with no conversation can still receive its first
        // delegated binding. Its new snapshot must use a readable tree format.
        let mut header = super::read_header(&session.path).unwrap();
        let super::SessionEntry::Session {
            version: stored_version,
            ..
        } = &mut header
        else {
            panic!("expected session header");
        };
        *stored_version = version;
        fs::write(
            &session.path,
            format!("{}\n", serde_json::to_string(&header).unwrap()),
        )
        .unwrap();
        // Prime the tree cache before replacing the header.
        session.cache_loaded_tree(SessionTree::load(&session.path).unwrap());
        assert_eq!(session.delegated_agent_id().unwrap(), None);
        let snapshot = session
            .snapshot_for_resume(
                ModelIdentity::new("test", "test", "selected"),
                "cache".into(),
            )
            .unwrap()
            .with_metadata("test-selection", "selected");
        session
            .rebind_fresh_delegated_agent("restricted-child", "new-fingerprint", Some(&snapshot))
            .unwrap();
        let expected_identity = Some(("restricted-child".into(), "new-fingerprint".into()));
        assert_eq!(session.stored_agent_identity().unwrap(), expected_identity);
        assert_eq!(
            session.delegated_agent_id().unwrap(),
            Some("restricted-child".into())
        );
        assert_eq!(
            session.session_metadata("test-selection").unwrap(),
            Some("selected".into())
        );
        // A restart before any turn checkpoint must see both binding halves.
        let (reopened, _) =
            Session::open_by_id_with_histories_in_root(root.path(), cwd.path(), session.id())
                .unwrap();
        assert_eq!(reopened.stored_agent_identity().unwrap(), expected_identity);
        assert_eq!(
            reopened.session_metadata("test-selection").unwrap(),
            Some("selected".into())
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
        assert!(session
            .rebind_fresh_delegated_agent("root", "old", /*initial_snapshot*/ None)
            .is_err());
        assert_eq!(fs::read(&session.path).unwrap(), before);
        let (reopened, histories) =
            Session::open_by_id_with_histories_in_root(root.path(), cwd.path(), session.id())
                .unwrap();
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
}
