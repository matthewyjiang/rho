//! Durable job records: one JSON file per job in
//! `<daemon state dir>/jobs/<job id>.json`, the state dir being
//! `<rho home>/daemon`.
//!
//! Records live in the rho home rather than the daemon's runtime directory,
//! which is a tmpfs on Linux and would lose them at reboot. The directory
//! sits in the private state dir and each file is replaced atomically (a temp file in the
//! same directory, then a rename), so a reader never sees a torn record. A
//! file that does not parse as a record named for its own id is logged and
//! skipped: one bad file never hides the others or stops the daemon.

use std::{
    fs, io,
    os::unix::fs::DirBuilderExt,
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
    /// The store in the daemon's `state_dir`, its directory created 0700.
    /// Records name the user's workspaces and sessions; they stay private
    /// because the store sits inside the state dir, which taking the daemon
    /// lock checked is private.
    pub(in crate::app::daemon_host) fn open(state_dir: &Path) -> io::Result<Self> {
        let dir = state_dir.join("jobs");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
        Ok(Self { dir })
    }

    /// Replaces `record`'s file. The file is flushed before the rename and
    /// the directory after it: a later daemon trusts the record to say what
    /// this one ran.
    pub(in crate::app::daemon_host) fn write(&self, record: &JobRecord) -> io::Result<()> {
        let mut contents = serde_json::to_vec_pretty(record)?;
        contents.push(b'\n');
        write_bytes_atomically_with_durability(
            &self.path_of(&record.id),
            &contents,
            WriteDurability::Durable,
        )?;
        fs::File::open(&self.dir)?.sync_all()
    }

    /// Deletes `id`'s file, if it has one, and flushes the directory, so a
    /// later daemon does not list the job.
    pub(in crate::app::daemon_host) fn remove(&self, id: &JobId) -> io::Result<()> {
        match fs::remove_file(self.path_of(id)) {
            Ok(()) => fs::File::open(&self.dir)?.sync_all(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Every record in the store, in no particular order.
    pub(in crate::app::daemon_host) fn load(&self) -> io::Result<Vec<JobRecord>> {
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
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
