//! Step checkpoint for one Rho agent attempt.
//!
//! The SDK saves the agent session through [`SessionStore`] before each
//! provider request and before each batch of tool calls. The file is replaced
//! atomically, so a reader sees one complete snapshot. It lives in the attempt
//! directory and is not registered in the session catalog.

use std::path::{Path, PathBuf};

use rho_sdk::{SessionId, SessionSnapshot, SessionStore, SessionStoreFuture};

use super::{
    attempt_directory, open_private_file_beneath, write_file_beneath, AttemptNumber,
    TaskInstanceId, WorkflowError, WorkflowResult,
};

const FILE_NAME: &str = "session.json";

/// The saved agent session of one attempt, beneath its run directory.
#[derive(Clone, Debug)]
pub(crate) struct AttemptCheckpoint {
    run_directory: PathBuf,
    relative: PathBuf,
}

impl AttemptCheckpoint {
    pub(crate) fn new(run_directory: &Path, node: &TaskInstanceId, attempt: AttemptNumber) -> Self {
        Self {
            run_directory: run_directory.to_path_buf(),
            // An empty root yields the attempt path relative to the run directory.
            relative: attempt_directory(Path::new(""), node, attempt)
                .join("agent")
                .join(FILE_NAME),
        }
    }

    /// Reads the saved snapshot, or `None` if the attempt never saved one.
    pub(crate) fn read(&self) -> WorkflowResult<Option<SessionSnapshot>> {
        let mut file = match open_private_file_beneath(&self.run_directory, &self.relative, false) {
            Ok(file) => file,
            Err(WorkflowError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None)
            }
            Err(error) => return Err(error),
        };
        let mut json = String::new();
        std::io::Read::read_to_string(&mut file, &mut json)?;
        SessionSnapshot::from_json(&json)
            .map(Some)
            .map_err(|error| WorkflowError::Corrupt {
                path: self.run_directory.join(&self.relative),
                reason: error.to_string(),
            })
    }

    /// Removes the saved transcript once its attempt can no longer continue.
    pub(crate) fn discard(&self) -> WorkflowResult<()> {
        match std::fs::remove_file(self.run_directory.join(&self.relative)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl SessionStore for AttemptCheckpoint {
    fn load<'a>(&'a self, id: &'a SessionId) -> SessionStoreFuture<'a, Option<SessionSnapshot>> {
        Box::pin(async move {
            Ok(self
                .read()
                .map_err(persistence_error)?
                .filter(|snapshot| snapshot.session_id() == id))
        })
    }

    fn save<'a>(&'a self, snapshot: SessionSnapshot) -> SessionStoreFuture<'a, ()> {
        Box::pin(async move {
            let json = snapshot.to_json()?;
            write_file_beneath(&self.run_directory, &self.relative, json.as_bytes())
                .map_err(persistence_error)
        })
    }
}

fn persistence_error(error: WorkflowError) -> rho_sdk::Error {
    rho_sdk::Error::Persistence {
        message: error.to_string(),
    }
}
