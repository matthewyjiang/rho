//! Durable PR records: one JSON file per session in
//! `<daemon state dir>/pull-requests/<session id>.json`, the state dir being
//! `<rho home>/daemon`. The daemon lock guards the state dir, so one daemon
//! writes these at a time.
//!
//! Each file is replaced atomically (a temp file in the same directory, then
//! a rename), as job records are (`daemon_host/jobs/store.rs`), so a reader
//! never sees a torn record. A file that does not parse as a record of this
//! format named for its own session is logged and skipped: one bad file
//! never hides the others or stops the daemon.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use rho_sdk::SessionId;
use serde::{Deserialize, Serialize};

use super::record::SessionRecord;
use crate::config_writer::{write_bytes_atomically_with_durability, WriteDurability};

/// The suffix of a record file. The atomic writer's temp files end in
/// `.tmp`, so a crash mid-write leaves nothing the loader reads.
const RECORD_SUFFIX: &str = ".json";

/// The format of a record file. A change to [`SessionRecord`]'s shape that
/// an older rho would misread bumps it; a file of another version is
/// skipped rather than misread.
const RECORD_VERSION: u32 = 1;

/// The directory of PR records.
#[derive(Clone, Debug)]
pub(crate) struct PullRequestStore {
    dir: PathBuf,
}

/// A record file as written.
#[derive(Serialize)]
struct RecordFileOut<'a> {
    version: u32,
    session: &'a SessionId,
    record: &'a SessionRecord,
}

/// A record file as read, once its version is known to be
/// [`RECORD_VERSION`].
#[derive(Deserialize)]
struct RecordFileIn {
    session: SessionId,
    record: SessionRecord,
}

/// Just the version of a record file, read first, so a file of another
/// version is told apart from a broken one.
#[derive(Deserialize)]
struct RecordVersion {
    version: u32,
}

impl PullRequestStore {
    /// The store in the daemon's `state_dir`, its directory created private:
    /// records name the user's sessions and repositories.
    pub(crate) fn open(state_dir: &Path) -> io::Result<Self> {
        let dir = state_dir.join("pull-requests");
        crate::daemon::create_private_dir(&dir)?;
        Ok(Self { dir })
    }

    /// Every session's record, in no particular order. An unreadable file
    /// is logged and skipped, and so is an unreadable directory: the
    /// daemon runs on without the records it cannot read.
    pub(crate) fn load_all(&self) -> Vec<(SessionId, SessionRecord)> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(dir = %self.dir.display(), %error, "could not list pull request records");
                return Vec::new();
            }
        };
        let mut records = Vec::new();
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    tracing::warn!(dir = %self.dir.display(), %error, "could not list pull request records");
                    continue;
                }
            };
            let Some(stem) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(RECORD_SUFFIX))
            else {
                continue;
            };
            match read_record(&path, stem) {
                Ok(record) => records.push(record),
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "skipping unreadable pull request record");
                }
            }
        }
        records
    }

    /// Replaces `session`'s file with `record`, or deletes it when the
    /// record [is empty](SessionRecord::is_empty). The file is flushed
    /// before the rename and the directory after it: queued news must
    /// survive a crash to be delivered at least once.
    ///
    /// # Errors
    ///
    /// When writing fails, or `InvalidInput` when `session` cannot name a
    /// file in the store (see `file_name`).
    pub(crate) fn save(&self, session: &SessionId, record: &SessionRecord) -> io::Result<()> {
        if record.is_empty() {
            return self.forget(session);
        }
        let file = RecordFileOut {
            version: RECORD_VERSION,
            session,
            record,
        };
        let mut contents = serde_json::to_vec_pretty(&file)?;
        contents.push(b'\n');
        write_bytes_atomically_with_durability(
            &self.path_of(session)?,
            &contents,
            WriteDurability::Durable,
        )?;
        crate::daemon::sync_dir(&self.dir)
    }

    /// Deletes `session`'s file, if it has one, and flushes the directory,
    /// so a later daemon does not load it. Called when the session is
    /// deleted.
    pub(crate) fn forget(&self, session: &SessionId) -> io::Result<()> {
        match fs::remove_file(self.path_of(session)?) {
            Ok(()) => crate::daemon::sync_dir(&self.dir),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn path_of(&self, session: &SessionId) -> io::Result<PathBuf> {
        let name = file_name(session.as_str()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("session id {session:?} cannot name a pull request record file"),
            )
        })?;
        Ok(self.dir.join(name))
    }
}

/// The file name of `session`'s record, or `None` when the id holds
/// anything but ASCII letters, digits, `-` and `_`. Session ids are UUIDs;
/// the check keeps any other id from naming a path outside the store, a
/// hidden file, or a temp file.
fn file_name(session: &str) -> Option<String> {
    let plain = !session.is_empty()
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    plain.then(|| format!("{session}{RECORD_SUFFIX}"))
}

/// Why a record file was skipped.
#[derive(Debug, thiserror::Error)]
enum UnreadableRecord {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("the file is in format version {found}, which this rho does not read")]
    Version { found: u32 },
    #[error("the file name is not a session id rho writes")]
    Foreign,
    #[error("the file is named for session {name}, but the record is session {session}")]
    Misnamed { name: String, session: SessionId },
}

/// The record at `path`, whose file stem is `stem`. A record must be named
/// for its own session, so the file a save rewrites is the one it was
/// loaded from.
fn read_record(path: &Path, stem: &str) -> Result<(SessionId, SessionRecord), UnreadableRecord> {
    if file_name(stem).is_none() {
        return Err(UnreadableRecord::Foreign);
    }
    let contents = fs::read(path)?;
    let RecordVersion { version } = serde_json::from_slice(&contents)?;
    if version != RECORD_VERSION {
        return Err(UnreadableRecord::Version { found: version });
    }
    let RecordFileIn { session, record } = serde_json::from_slice(&contents)?;
    if session.as_str() != stem {
        return Err(UnreadableRecord::Misnamed {
            name: stem.to_owned(),
            session,
        });
    }
    Ok((session, record))
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
