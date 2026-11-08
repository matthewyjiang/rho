//! The files a T3 wake watcher shares with its parent (see [`super`]), in
//! one directory of the daemon's Rho home: per watcher, the signal its
//! parent writes once a result is ready and the lock the parent holds while
//! it lives; per watcher session, the record of its wait. Every file is
//! private to the user, since a result names the command that finished.

use std::{
    ffi::OsStr,
    fs::{File, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use rho_sdk::CancellationToken;

/// How often a watcher checks for its signal. T3 takes seconds to start the
/// woken turn, so a half-second poll adds little latency and no real load.
const SIGNAL_POLL: Duration = Duration::from_millis(500);

/// The watcher's result when its parent ended before any result was ready.
pub(super) const PARENT_ENDED: &str = "the Rho session that started this background work ended \
                                       before it finished";

/// How long a watcher's files outlive their use. T3 Code asks a watcher to
/// continue only when it restarted before it saw the result, and it asks as
/// it starts again, so a record matters until T3's next start. A month
/// covers a machine left off for weeks; past that a lost result costs one
/// model turn in the watcher, as before records existed. Files of a parent
/// that still lives never expire.
const EXPIRED_AFTER: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Where parents write signals and watchers record their waits: `t3-wakes`
/// under the daemon's Rho home, where a restarted daemon still finds its
/// watchers' records. Resolved once, as the daemon starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::daemon_host::sessions) struct WakeDir(PathBuf);

impl WakeDir {
    pub(in crate::app::daemon_host::sessions) fn in_home(rho_home: &Path) -> Self {
        Self(rho_home.join("t3-wakes"))
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

/// The files one watcher shares with its parent: the signal the parent
/// writes when a result is ready, and a lock the parent holds while it
/// lives. The token is a UUID, so a watcher prompt cannot name a path
/// outside the directory.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app::daemon_host::sessions) struct WakeSignal {
    dir: WakeDir,
    token: String,
}

impl WakeSignal {
    /// A fresh signal, with the parent's lock on it held by the returned
    /// file. Creates the directory, private to the user, when missing.
    pub(super) fn create(dir: &WakeDir) -> io::Result<(Self, File)> {
        crate::subagent::ensure_private_directory(dir.path())?;
        let signal = Self {
            dir: dir.clone(),
            token: uuid::Uuid::new_v4().to_string(),
        };
        let lock = private_file(&signal.lock_path())?;
        lock.lock()?;
        Ok((signal, lock))
    }

    /// The signal `token` names in `dir`, if the token is well formed.
    pub(super) fn parse(dir: &WakeDir, token: &str) -> Option<Self> {
        Some(Self {
            dir: dir.clone(),
            token: uuid::Uuid::parse_str(token.trim()).ok()?.to_string(),
        })
    }

    /// The signal watcher session `session_id` waited on in an earlier
    /// turn, if any, in `dir`. T3 Code sends a thread that a restart cut off
    /// a prompt to continue, which must resume the wait, or repeat its
    /// result, instead of starting a model turn: a wait a restart cut off
    /// stays recorded, whether it was cancelled or still runs (see
    /// [`Self::wait`]).
    pub(in crate::app::daemon_host::sessions) fn recorded(
        dir: &WakeDir,
        session_id: &str,
    ) -> Option<Self> {
        let token = WaitRecord::of(dir, session_id)?.read()?.token;
        Self::parse(dir, &token)
    }

    /// Drops what watcher session `session_id` recorded in `dir`, waiting
    /// or finished, with the signal and lock file it names.
    pub(in crate::app::daemon_host::sessions) fn forget(dir: &WakeDir, session_id: &str) {
        if let Some(signal) = Self::recorded(dir, session_id) {
            signal.cleanup();
        }
        if let Some(record) = WaitRecord::of(dir, session_id) {
            record.remove();
        }
    }

    pub(super) fn token(&self) -> &str {
        &self.token
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join(&self.token)
    }

    fn lock_path(&self) -> PathBuf {
        self.path().with_extension("lock")
    }

    /// Writes `summary` whole: a temp file renamed into place, so a polling
    /// watcher never reads a partial file.
    pub(super) fn fire(&self, summary: &str) -> io::Result<()> {
        write_whole(&self.path(), &self.path().with_extension("tmp"), summary)
    }

    pub(super) fn remove(&self) {
        let _ = std::fs::remove_file(self.path());
    }

    pub(super) fn cleanup(&self) {
        self.remove();
        let _ = std::fs::remove_file(self.lock_path());
    }

    /// Whether the parent let go of its lock: the daemon unloaded or
    /// deleted its session, or died, as the OS then releases its locks.
    /// The lock is per open file, so a watcher in the parent's own daemon
    /// sees it held.
    pub(super) fn parent_ended(&self) -> bool {
        released(&self.lock_path())
    }

    /// Waits, as watcher session `session_id`, for the parent's signal or
    /// for the parent to end, and cleans up, returning the watcher's result.
    ///
    /// The wait and then its result stay recorded under the session, so a
    /// watcher whose result T3 Code has not seen repeats it on the next
    /// prompt (see [`Self::recorded`]). `None` when `cancel` trips first;
    /// the wait stays recorded, so the session's next prompt resumes it.
    pub(in crate::app::daemon_host::sessions) async fn wait(
        &self,
        session_id: &str,
        cancel: &CancellationToken,
    ) -> Option<String> {
        let record = WaitRecord::of(&self.dir, session_id);
        if let Some(record) = &record {
            if let Some(summary) = record.read().and_then(|recorded| {
                (recorded.token == self.token)
                    .then_some(recorded.summary)
                    .flatten()
            }) {
                return Some(summary);
            }
            prune_expired(&self.dir);
            // A watcher left unrecorded only loses restart recovery.
            let _ = record.write(&self.token, /*summary*/ None);
        }
        let summary = loop {
            if let Ok(summary) = std::fs::read_to_string(self.path()) {
                break summary;
            }
            if self.parent_ended() {
                break PARENT_ENDED.into();
            }
            tokio::select! {
                () = cancel.cancelled() => return None,
                () = tokio::time::sleep(SIGNAL_POLL) => {}
            }
        };
        // The signal and lock go only once the record holds the result: a
        // later wait repeats it from either, and from neither would report
        // that the parent ended.
        let recorded = record
            .as_ref()
            .is_none_or(|record| record.write(&self.token, Some(&summary)));
        if recorded {
            self.cleanup();
        }
        Some(summary)
    }
}

/// One watcher session's file naming the signal it waits on, and, once the
/// wait ends, its result: the token on the first line, then the result.
struct WaitRecord {
    path: PathBuf,
}

struct RecordedWait {
    token: String,
    /// The result, once the wait ended.
    summary: Option<String>,
}

impl WaitRecord {
    /// `None` for a session id that is not a UUID, which Rho never creates,
    /// so a host's session id cannot name a path outside `dir`.
    fn of(dir: &WakeDir, session_id: &str) -> Option<Self> {
        let session_id = uuid::Uuid::parse_str(session_id).ok()?;
        Some(Self {
            path: dir.path().join(format!("{session_id}.watcher")),
        })
    }

    fn read(&self) -> Option<RecordedWait> {
        std::fs::read_to_string(&self.path)
            .ok()
            .map(|contents| Self::parse(&contents))
    }

    fn parse(contents: &str) -> RecordedWait {
        let (token, summary) = match contents.split_once('\n') {
            Some((token, summary)) => (token, Some(summary.to_owned())),
            None => (contents, None),
        };
        RecordedWait {
            token: token.to_owned(),
            summary,
        }
    }

    /// Writes whole, through a temp file, so a crash never leaves a waiting
    /// record half turned into a finished one. Returns whether it wrote;
    /// a failure is logged.
    fn write(&self, token: &str, summary: Option<&str>) -> bool {
        let contents = match summary {
            Some(summary) => format!("{token}\n{summary}"),
            None => token.to_owned(),
        };
        let written = write_whole(
            &self.path,
            &self.path.with_extension("watcher-tmp"),
            &contents,
        );
        if let Err(error) = &written {
            tracing::warn!(%error, "could not record the T3 wake watcher's wait");
        }
        written.is_ok()
    }

    fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Removes the files in `dir` older than [`EXPIRED_AFTER`] that no wait can
/// use any more: finished records; the waiting records, signals, and lock
/// files of parents that ended; and temp files a crash left. Only old files
/// go, so a lock a parent created but has not locked yet stays.
fn prune_expired(dir: &WakeDir) {
    let Ok(entries) = std::fs::read_dir(dir.path()) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let expired = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > EXPIRED_AFTER));
        if expired && unusable(dir, &path) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Whether no wait can use the file at `path` in `dir`, as named by
/// [`WakeSignal`] and [`WaitRecord`]. A record's or a signal's parent ended
/// once its lock is free or gone.
fn unusable(dir: &WakeDir, path: &Path) -> bool {
    let parent_ended =
        |token: &str| WakeSignal::parse(dir, token).is_none_or(|signal| signal.parent_ended());
    match path.extension().and_then(OsStr::to_str) {
        Some("watcher") => std::fs::read_to_string(path).is_ok_and(|contents| {
            let recorded = WaitRecord::parse(&contents);
            recorded.summary.is_some() || parent_ended(&recorded.token)
        }),
        Some("lock") => released(path),
        Some("tmp" | "watcher-tmp") => true,
        // A signal is named by its bare token.
        None => path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(parent_ended),
        Some(_) => false,
    }
}

/// Whether nobody holds the lock at `path`, or it is gone.
pub(super) fn released(path: &Path) -> bool {
    match File::open(path) {
        Ok(lock) => lock.try_lock().is_ok(),
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    }
}

/// Opens `path` for writing, emptied, and created private to the user.
fn private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

/// Writes `contents` to `path` whole: to the private file `staging`, then
/// renamed into place, so a reader never sees a partial file.
fn write_whole(path: &Path, staging: &Path, contents: &str) -> io::Result<()> {
    private_file(staging)?.write_all(contents.as_bytes())?;
    std::fs::rename(staging, path)
}

#[cfg(test)]
#[path = "signal_tests.rs"]
mod tests;
