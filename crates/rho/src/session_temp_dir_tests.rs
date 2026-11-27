use pretty_assertions::assert_eq;
use rho_sdk::SessionId;

use super::SessionTempDir;

// Covers: a session's temp dir is the same on every opening, another
// session's is not, an id that holds a path still names a plain directory
// under the per-user parent, and a removed dir is made again.
// Owner: unit (session temp dir)
#[test]
fn a_sessions_temp_dir_is_its_own_and_stable() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join(super::parent_name());
    let prepare = |id: &str| {
        SessionTempDir::prepare(root.path(), &SessionId::from_string(id).unwrap()).unwrap()
    };
    let first = prepare("a1b2-c3");
    let escaping = prepare("../escape");
    std::fs::remove_dir(first.path()).unwrap();
    first.ensure().unwrap();

    assert_eq!(
        (
            prepare("a1b2-c3") == first,
            prepare("other") == first,
            escaping.path().parent() == Some(parent.as_path()),
            first.path().is_dir(),
        ),
        (true, false, true, true)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &std::path::Path| path.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!([mode(first.path()), mode(&parent)], [0o700, 0o700]);
    }
}

// Covers: a parent another user could write into is refused, so a
// session's scratch files never land where someone else can read or
// replace them.
// Owner: unit (session temp dir)
#[cfg(unix)]
#[test]
fn a_parent_others_can_write_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join(super::parent_name());
    std::fs::create_dir(&parent).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();

    assert!(SessionTempDir::prepare(root.path(), &SessionId::new()).is_err());
}
