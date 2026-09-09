//! Machine-local consent, never read from project config or conversation history.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};
use rho_sdk::SessionId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComputerUsePreference {
    Disabled,
    Enabled,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreferenceFile {
    enabled: bool,
}

impl ComputerUsePreference {
    /// Persist only explicit human consent or revocation. Connection failures,
    /// plan mode, and session shutdown must not change this preference.
    pub(crate) fn save(self) -> anyhow::Result<()> {
        self.save_to(&preference_path(&crate::paths::rho_dir()?)?)
    }

    /// Resume only this session's consent. Missing legacy records are disabled,
    /// regardless of the default or anything stored in the transcript.
    pub(crate) fn load_session(session_id: &SessionId) -> anyhow::Result<Self> {
        Self::load_from(&session_path(&crate::paths::rho_dir()?, session_id)?)
    }

    /// Persist explicit consent or revocation for this session independently of
    /// the new-session default. Callers must not enable access after a save error.
    pub(crate) fn save_session(self, session_id: &SessionId) -> anyhow::Result<()> {
        self.save_to(&session_path(&crate::paths::rho_dir()?, session_id)?)
    }

    /// Snapshot the default for a genuinely new session before enabling access.
    /// Call only once per new ID. Resume must use `load_session`: absence means
    /// disabled, so ordinary off-by-default sessions need no preference file.
    pub(crate) fn initialize_session(session_id: &SessionId) -> anyhow::Result<Self> {
        Self::initialize_session_in(&crate::paths::rho_dir()?, session_id)
    }

    fn initialize_session_in(root: &Path, session_id: &SessionId) -> anyhow::Result<Self> {
        let path = session_path(root, session_id)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => return Self::load_from(&path),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        }
        let preference = Self::load_from(&preference_path(root)?)?;
        if preference == Self::Enabled {
            preference.save_to(&path)?;
        }
        Ok(preference)
    }

    fn load_from(path: &Path) -> anyhow::Result<Self> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::Disabled),
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        if !metadata.is_file() {
            bail!(
                "computer preference is not a regular file: {}",
                path.display()
            );
        }
        let contents =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let preference: PreferenceFile =
            toml::from_str(&contents).with_context(|| format!("parse {}", path.display()))?;
        Ok(if preference.enabled {
            Self::Enabled
        } else {
            Self::Disabled
        })
    }

    fn save_to(self, path: &Path) -> anyhow::Result<()> {
        let contents = toml::to_string(&PreferenceFile {
            enabled: self == Self::Enabled,
        })?;
        crate::config_writer::write_atomically(path, &contents)
            .with_context(|| format!("save {}", path.display()))
    }
}

fn preference_path(root: &Path) -> anyhow::Result<PathBuf> {
    // A relative RHO_HOME must not turn a checked-out file into desktop consent.
    if !root.is_absolute() {
        bail!(
            "computer preference requires an absolute Rho home: {}",
            root.display()
        );
    }
    Ok(root.join("computer-use.toml"))
}

fn session_path(root: &Path, session_id: &SessionId) -> anyhow::Result<PathBuf> {
    preference_path(root)?;
    // SessionId only rejects empty strings. Validate before using it in a path.
    let id = uuid::Uuid::parse_str(session_id.as_str())
        .context("computer preference requires a UUID session identifier")?;
    if id.to_string() != session_id.as_str() {
        bail!("computer preference requires a canonical UUID session identifier");
    }
    let directory = root.join("computer-use-sessions");
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) if !metadata.is_dir() => bail!(
            "computer preference session storage is not a directory: {}",
            directory.display()
        ),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", directory.display()));
        }
    }
    Ok(directory.join(format!("{id}.toml")))
}

#[cfg(test)]
#[path = "preference_tests.rs"]
mod tests;
