use std::path::PathBuf;
#[cfg(test)]
use std::sync::Arc;

use crate::config::Config;

/// Loads and persists the application configuration at one configured path.
#[derive(Clone, Debug)]
pub(crate) struct ConfigRepository {
    path: Option<PathBuf>,
    #[cfg(test)]
    _temp_dir: Option<Arc<tempfile::TempDir>>,
}

impl ConfigRepository {
    pub(crate) fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            #[cfg(test)]
            _temp_dir: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn temporary_for_tests() -> anyhow::Result<Self> {
        let temp_dir = Arc::new(tempfile::tempdir()?);
        Ok(Self {
            path: Some(temp_dir.path().join("config.toml")),
            _temp_dir: Some(temp_dir),
        })
    }

    pub(crate) fn configured_path(&self) -> anyhow::Result<PathBuf> {
        self.path
            .clone()
            .map(Ok)
            .unwrap_or_else(Config::default_path)
    }

    pub(crate) fn load(&self) -> anyhow::Result<Config> {
        Config::load(self.path.clone())
    }

    /// Saves every value of `config` that differs from the file, keeping
    /// the rest of the file as written.
    pub(crate) fn save(&self, config: &Config) -> anyhow::Result<()> {
        self.update(|saved| *saved = config.clone())
    }

    /// Loads the saved config, applies one typed mutation, and writes back
    /// only the keys it changed before returning.
    ///
    /// The file's edit lock is held from load to write; a concurrent writer
    /// makes this fail instead of waiting. Nothing is written when the
    /// mutation leaves an invalid config.
    pub(crate) fn update<T>(&self, update: impl FnOnce(&mut Config) -> T) -> anyhow::Result<T> {
        Config::edit(&self.configured_path()?, update)
    }
}

#[cfg(test)]
#[path = "config_repository_tests.rs"]
mod tests;
