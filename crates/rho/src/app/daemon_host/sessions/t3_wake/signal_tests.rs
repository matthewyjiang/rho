use std::time::SystemTime;

use pretty_assertions::assert_eq;

use super::*;

/// The daemon session id of the watcher reading a signal.
const WATCHER_SESSION: &str = "5a7c9e1b-3d4f-4a6b-8c0d-2e4f6a8b0c1d";

/// A wake directory, created, in a fresh Rho home.
fn wake_dir() -> (tempfile::TempDir, WakeDir) {
    let home = tempfile::tempdir().unwrap();
    let dir = WakeDir::in_home(home.path());
    crate::subagent::ensure_private_directory(dir.path()).unwrap();
    (home, dir)
}

fn set_modified(path: &Path, modified: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// Covers: a watcher starting its wait prunes the old files no wait can use,
// finished records and the records, signals, and locks of parents that
// ended, and keeps those of a parent that lives, which a watcher still
// reads or resumes, and fresh finished records, which T3 may still ask to
// repeat. Pruning too much brings back model turns in watchers, or reports
// a live parent ended; too little grows the directory by files per watcher
// forever. Owner: daemon T3 wake watcher files.
#[tokio::test]
async fn starting_a_wait_prunes_only_old_unusable_files() {
    let (_home, dir) = wake_dir();
    let (live, ended) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let live_lock = dir.path().join(format!("{live}.lock"));
    let held = crate::daemon::create_private_file(&live_lock).unwrap();
    held.lock().unwrap();
    let old = SystemTime::now() - EXPIRED_AFTER - Duration::from_secs(60);
    let record = || format!("{}.watcher", uuid::Uuid::new_v4());
    let files = [
        (
            "old finished record",
            record(),
            format!("{live}\nmake exited 0"),
            old,
        ),
        (
            "old waiting record of a live parent",
            record(),
            live.to_string(),
            old,
        ),
        (
            "old waiting record of an ended parent",
            record(),
            ended.to_string(),
            old,
        ),
        (
            "new finished record",
            record(),
            format!("{ended}\nmake exited 0"),
            SystemTime::now(),
        ),
        (
            "old signal of a live parent",
            live.to_string(),
            "make exited 0".into(),
            old,
        ),
        (
            "old signal of an ended parent",
            ended.to_string(),
            "make exited 0".into(),
            old,
        ),
        (
            "old lock of an ended parent",
            format!("{ended}.lock"),
            String::new(),
            old,
        ),
        (
            "old staging file",
            format!("{ended}.tmp"),
            String::new(),
            old,
        ),
    ]
    .map(|(name, file, contents, modified)| {
        let path = dir.path().join(file);
        std::fs::write(&path, contents).unwrap();
        set_modified(&path, modified);
        (name, path)
    });
    set_modified(&live_lock, old);
    let signal = WakeSignal::parse(&dir, &uuid::Uuid::new_v4().to_string()).unwrap();
    signal.fire("cargo test exited 0").unwrap();

    let summary = signal
        .wait(WATCHER_SESSION, &CancellationToken::new())
        .await;

    assert_eq!(
        (
            summary.as_deref(),
            files.map(|(name, path)| (name, path.exists())),
            live_lock.exists(),
        ),
        (
            Some("cargo test exited 0"),
            [
                ("old finished record", false),
                ("old waiting record of a live parent", true),
                ("old waiting record of an ended parent", false),
                ("new finished record", true),
                ("old signal of a live parent", true),
                ("old signal of an ended parent", false),
                ("old lock of an ended parent", false),
                ("old staging file", false),
            ],
            true,
        )
    );
}

// Covers: when the finished record cannot be written, the watcher keeps the
// parent's signal, so a restarted watcher still repeats the real result.
// Dropping it first turns finished work into the parent-ended notice.
// Owner: daemon T3 wake watcher files.
#[tokio::test]
async fn failed_result_record_keeps_the_signal_for_the_restarted_watcher() {
    let (_home, dir) = wake_dir();
    let signal = WakeSignal::parse(&dir, &uuid::Uuid::new_v4().to_string()).unwrap();
    signal.fire("cargo test exited 0").unwrap();
    // A directory where the record stages its write makes every write fail.
    std::fs::create_dir(dir.path().join(format!("{WATCHER_SESSION}.watcher-tmp"))).unwrap();

    let first = signal
        .wait(WATCHER_SESSION, &CancellationToken::new())
        .await;
    let restarted = signal
        .wait(WATCHER_SESSION, &CancellationToken::new())
        .await;

    assert_eq!(
        (first.as_deref(), restarted.as_deref()),
        (Some("cargo test exited 0"), Some("cargo test exited 0"))
    );
}

// Covers: the wake directory and every file in it are private to the user:
// a signal and a record hold the finished command's line, which may carry
// secrets, and the directory sits in a home other users may read.
// Owner: daemon T3 wake watcher files.
#[cfg(unix)]
#[tokio::test]
async fn wake_files_are_private() {
    let home = tempfile::tempdir().unwrap();
    let dir = WakeDir::in_home(home.path());
    let (signal, _lock) = WakeSignal::create(&dir).unwrap();
    signal.fire("deploy --token secret exited 0").unwrap();
    let modes_before_wait = (
        mode(dir.path()),
        mode(&signal.lock_path()),
        mode(&signal.path()),
    );
    // The wait takes the signal and lock, and records the result.
    signal
        .wait(WATCHER_SESSION, &CancellationToken::new())
        .await
        .unwrap();
    let record = WaitRecord::of(&dir, WATCHER_SESSION).unwrap();

    assert_eq!(
        (modes_before_wait, mode(&record.path)),
        ((0o700, 0o600, 0o600), 0o600)
    );
}
