//! `rho/hooks/file/*`: the user's hooks file as text, read and replaced
//! whole under a revision, the SHA-256 of its bytes. A write lands only text
//! a session would load, so a copied file never leaves the daemon running no
//! hooks at all. Sessions read the file as they open and on
//! `rho/session/hooks/reload`, so a write changes no loaded session.
//!
//! A write holds the file's edit lock from its revision check through its
//! write, as a `config.toml` edit does, so no other rho process replaces
//! the file in between. A text editor takes no lock, so its save can still
//! land in that window, which lasts microseconds.

use std::{
    io,
    path::{Path, PathBuf},
};

use rho_protocol::{
    ConfigRevision, ErrorCode, HooksFileGetResponse, HooksFileSetParams, HooksFileSetResponse,
    RpcError,
};
use sha2::{Digest, Sha256};

/// The revision of an absent file, which no hex digest equals, so it differs
/// from an empty file's.
const ABSENT: &str = "absent";

/// The daemon's user hooks file.
pub(super) struct HooksFileStore {
    path: PathBuf,
    /// Held from a write's revision check through its write, so this
    /// daemon's writes wait for each other instead of finding the edit lock
    /// held.
    writes: tokio::sync::Mutex<()>,
}

impl HooksFileStore {
    pub(super) fn new(rho_home: &Path) -> Self {
        Self {
            path: crate::hooks::user_hooks_path(rho_home),
            writes: tokio::sync::Mutex::new(()),
        }
    }

    /// Answers `rho/hooks/file/get`.
    pub(super) fn get(&self) -> Result<HooksFileGetResponse, RpcError> {
        let bytes = self.read()?;
        let revision = revision(bytes.as_deref());
        let text = bytes
            .map(|bytes| {
                String::from_utf8(bytes).map_err(|_| {
                    RpcError::new(
                        ErrorCode::InternalError,
                        format!("{} is not UTF-8 text", self.path.display()),
                    )
                })
            })
            .transpose()?;
        let path = crate::paths::without_verbatim_prefix(self.path.clone());
        Ok(HooksFileGetResponse::new(path, revision, text))
    }

    /// Answers `rho/hooks/file/set`: checks the text, then, while the file
    /// is still at the params' revision, writes it atomically, or deletes the
    /// file when there is none.
    pub(super) async fn set(
        &self,
        params: HooksFileSetParams,
    ) -> Result<HooksFileSetResponse, RpcError> {
        if let Some(text) = &params.text {
            crate::hooks::check_user_hooks(&self.path, text)
                .map_err(|error| RpcError::new(ErrorCode::InvalidParams, error.to_string()))?;
        }
        let _writing = self.writes.lock().await;
        let _locked = self.lock()?;
        if revision(self.read()?.as_deref()) != params.revision {
            return Err(RpcError::new(
                ErrorCode::ConfigConflict,
                format!(
                    "{} changed since it was read; read it again",
                    self.path.display()
                ),
            ));
        }
        let written = match &params.text {
            Some(text) => crate::config_writer::write_bytes_atomically(&self.path, text.as_bytes()),
            None => match std::fs::remove_file(&self.path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                removed => removed,
            },
        };
        written.map_err(|error| {
            RpcError::new(
                ErrorCode::InternalError,
                format!("could not write {}: {error}", self.path.display()),
            )
        })?;
        Ok(HooksFileSetResponse::new(revision(
            params.text.as_deref().map(str::as_bytes),
        )))
    }

    /// The file's edit lock, which every rho writer of it takes. Held by
    /// another, it is a conflict the client can retry after reading again.
    fn lock(&self) -> Result<crate::config_writer::edit_lock::EditFileLock, RpcError> {
        crate::config_writer::edit_lock::acquire_edit_lock(&self.path).map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                RpcError::new(
                    ErrorCode::ConfigConflict,
                    format!("{} is being edited; read it again", self.path.display()),
                )
            } else {
                RpcError::new(
                    ErrorCode::InternalError,
                    format!("could not lock {}: {error}", self.path.display()),
                )
            }
        })
    }

    /// The file's bytes, `None` when there is no file.
    fn read(&self) -> Result<Option<Vec<u8>>, RpcError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(RpcError::new(
                ErrorCode::InternalError,
                format!("could not read {}: {error}", self.path.display()),
            )),
        }
    }
}

/// The revision of a file holding `bytes`, or of no file.
fn revision(bytes: Option<&[u8]>) -> ConfigRevision {
    ConfigRevision::new(bytes.map_or_else(
        || ABSENT.to_owned(),
        |bytes| hex::encode(Sha256::digest(bytes)),
    ))
}
