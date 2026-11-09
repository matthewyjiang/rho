//! What the settings store needs of a file: whether it changed since the
//! store last read it, its document, and the revision of what it holds.

use std::{collections::BTreeMap, io, os::unix::fs::MetadataExt, path::Path, time::SystemTime};

use rho_protocol::{ConfigRevision, SettingEntry, SettingKey};
use sha2::{Digest, Sha256};

use crate::config::ConfigDocument;

/// A file's state as `stat` reports it. Comparing two tells whether the
/// file was written in between, including by an editor that saves by
/// renaming a new file over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stat {
    Missing,
    File {
        modified: Option<SystemTime>,
        len: u64,
        inode: u64,
    },
    /// `stat` failed for another reason; reading the file reports why.
    Unreadable,
}

impl Stat {
    pub(super) fn of(path: &Path) -> Self {
        match std::fs::metadata(path) {
            Ok(metadata) => Self::File {
                modified: metadata.modified().ok(),
                len: metadata.len(),
                inode: metadata.ino(),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::Missing,
            Err(_) => Self::Unreadable,
        }
    }
}

/// Why a settings file could not be read as a document.
#[derive(Debug)]
pub(super) enum ReadError {
    Io(io::Error),
    /// Not valid TOML, so no change can be merged into it.
    Parse(String),
}

impl ReadError {
    pub(super) fn message(&self, path: &Path) -> String {
        match self {
            Self::Io(error) => format!("could not read {}: {error}", path.display()),
            Self::Parse(error) => format!("{} is not valid TOML: {error}", path.display()),
        }
    }
}

/// The document at `path`; a missing file is an empty one.
pub(super) fn read(path: &Path) -> Result<ConfigDocument, ReadError> {
    match std::fs::read_to_string(path) {
        Ok(text) => ConfigDocument::parse(&text)
            .map_err(|error| ReadError::Parse(error.to_string().trim_end().to_owned())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ConfigDocument::default()),
        Err(error) => Err(ReadError::Io(error)),
    }
}

/// The revision of `values`: the SHA-256 of their JSON, so equal values
/// have one revision across daemon restarts, and a hand edit that changes
/// only comments keeps it.
pub(super) fn revision(values: &BTreeMap<SettingKey, SettingEntry>) -> ConfigRevision {
    let json = serde_json::to_vec(values).expect("setting values encode as JSON");
    ConfigRevision::new(hex::encode(Sha256::digest(json)))
}
