//! Durable job records: one JSON file per job in
//! `<rho home>/daemon/jobs/<job id>.json`.
//!
//! Records live in the rho home rather than the daemon's runtime directory,
//! which is a tmpfs on Linux and would lose them at reboot. The directory is
//! private (0700) and each file is replaced atomically (a temp file in the
//! same directory, then a rename), so a reader never sees a torn record. A
//! file that does not parse as a record named for its own id is logged and
//! skipped: one bad file never hides the others or stops the daemon.

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};

use super::{JobId, JobRecord};
use crate::config_writer::{write_bytes_atomically_with_durability, WriteDurability};

/// The directory of job records.
#[derive(Clone, Debug)]
pub(in crate::app::daemon_host) struct JobStore {
    dir: PathBuf,
}

/// The suffix of a record file. The atomic writer's temp files end in
/// `.tmp`, so a crash mid-write leaves nothing the loader reads.
const RECORD_SUFFIX: &str = ".json";

impl JobStore {
    /// The store in `rho_home`.
    pub(in crate::app::daemon_host) fn in_rho_home(rho_home: &Path) -> Self {
        Self {
            dir: rho_home.join("daemon").join("jobs"),
        }
    }

    /// Replaces `record`'s file. Flushed before the rename: a later daemon
    /// trusts it to say what this one ran.
    pub(in crate::app::daemon_host) fn write(&self, record: &JobRecord) -> io::Result<()> {
        self.prepare_dir()?;
        let mut contents = serde_json::to_vec_pretty(record)?;
        contents.push(b'\n');
        write_bytes_atomically_with_durability(
            &self.path_of(&record.id),
            &contents,
            WriteDurability::Durable,
        )
    }

    /// Every record in the store, in no particular order. A missing
    /// directory holds none.
    pub(in crate::app::daemon_host) fn load(&self) -> io::Result<Vec<JobRecord>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut records = Vec::new();
        for entry in entries {
            let path = entry?.path();
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
                    tracing::warn!(path = %path.display(), %error, "skipping unreadable job record");
                }
            }
        }
        Ok(records)
    }

    fn path_of(&self, id: &JobId) -> PathBuf {
        self.dir.join(format!("{}{RECORD_SUFFIX}", id.as_str()))
    }

    /// Creates the directory 0700, or closes an existing one to group and
    /// others: records name the user's projects and sessions.
    fn prepare_dir(&self) -> io::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)?;
        fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))
    }
}

/// Why a record file was skipped.
#[derive(Debug, thiserror::Error)]
enum UnreadableRecord {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("the file is named for job {name}, but the record is job {id}")]
    Misnamed { name: String, id: JobId },
}

/// The record at `path`, whose file stem is `stem`. A record must be named
/// for its own id, so the file a transition rewrites is the one it was
/// loaded from.
fn read_record(path: &Path, stem: &str) -> Result<JobRecord, UnreadableRecord> {
    let record: JobRecord = serde_json::from_slice(&fs::read(path)?)?;
    if record.id.as_str() != stem {
        return Err(UnreadableRecord::Misnamed {
            name: stem.to_owned(),
            id: record.id,
        });
    }
    Ok(record)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
