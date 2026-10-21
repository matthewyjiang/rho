use std::{fs::File, time::Duration};

use super::{is_lock_busy, FileLock};

// Covers: a bounded acquire gives up with the busy error while another handle
// holds the lock, then succeeds once that holder is dropped.
// Owner: file lock
#[test]
fn bounded_acquire_reports_busy_until_the_holder_releases() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.lock");
    let open = || {
        File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap()
    };

    let holder = FileLock::acquire(open()).unwrap();
    let Err(busy) = FileLock::acquire_with_retry(open(), 2, Duration::ZERO) else {
        panic!("a second handle must not take a held lock");
    };
    assert!(is_lock_busy(&busy), "{busy}");

    drop(holder);
    assert!(FileLock::acquire_with_retry(open(), 0, Duration::ZERO).is_ok());
}
