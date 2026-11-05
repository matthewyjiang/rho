use pretty_assertions::assert_eq;
use rho_protocol::PROTOCOL_VERSION;
use tempfile::TempDir;

use super::*;

fn prepared_paths(root: &TempDir) -> DaemonPaths {
    let paths = DaemonPaths::resolve(root.path(), |_| None).unwrap();
    paths.prepare_runtime_dir().unwrap();
    paths
}

// Covers: a second daemon is refused while the first holds the lock and the
// error names the running daemon; the lock frees when the holder goes away.
// Owner: daemon single-instance lock.
#[test]
fn second_acquire_names_the_running_daemon_until_the_first_releases() {
    let root = TempDir::new().unwrap();
    let paths = prepared_paths(&root);
    let first = acquire(&paths).unwrap();
    let record = DaemonRecord {
        pid: 4242,
        version: "9.9.9".into(),
        protocol: PROTOCOL_VERSION,
        started_at: 1,
        socket: paths.socket.clone(),
    };
    write_record(&paths, &record).unwrap();

    let error = acquire(&paths).unwrap_err();

    let LockError::AlreadyRunning { record: reported } = error else {
        panic!("expected AlreadyRunning");
    };
    assert_eq!(reported, Some(record));
    assert!(is_held(&paths).unwrap());

    drop(first);
    assert!(!is_held(&paths).unwrap());
    acquire(&paths).unwrap();
}
