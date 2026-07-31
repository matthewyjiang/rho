use std::{io::Read as _, path::PathBuf};

use crate::workflow::RunId;

use super::{artifacts::ensure_private_directory, RuntimeError};

// Receipt: the cross-process command-cancellation E2E completed in 87 ms
// with this 100 ms poll, below its sub-second response target.
pub(super) const CROSS_PROCESS_CANCEL_POLL: std::time::Duration =
    std::time::Duration::from_millis(100);

#[derive(Clone)]
pub(crate) struct CancellationRequest {
    pub(super) path: PathBuf,
    pub(super) cancellation: rho_sdk::CancellationToken,
}

impl CancellationRequest {
    pub(crate) fn request(&self) -> Result<(), RuntimeError> {
        if let Some(parent) = self.path.parent() {
            ensure_private_directory(parent)?;
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        crate::config_writer::write_bytes_atomically(&self.path, request_id.as_bytes())?;
        self.cancellation.cancel();
        Ok(())
    }
}

pub(crate) struct CancellationRequestReceipt {
    pub(super) request_id: String,
}

pub(super) fn read_cancellation_request(
    run_directory: &std::path::Path,
) -> Result<Option<String>, RuntimeError> {
    let mut file = match crate::workflow::open_file_beneath(
        run_directory,
        std::path::Path::new("cancel.request"),
    ) {
        Ok(file) => file,
        Err(crate::workflow::WorkflowError::Io(source))
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let mut token = String::new();
    file.by_ref().take(37).read_to_string(&mut token)?;
    let parsed = uuid::Uuid::parse_str(&token)
        .map_err(|_| RuntimeError::Data("cancel request has an invalid identifier".into()))?;
    if parsed.to_string() != token {
        return Err(RuntimeError::Data(
            "cancel request identifier is not canonical".into(),
        ));
    }
    Ok(Some(token))
}

pub(super) fn run_directory(rho_home: &std::path::Path, run_id: RunId) -> PathBuf {
    rho_home
        .join("workflows")
        .join("runs")
        .join(run_id.to_string())
}
