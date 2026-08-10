use std::path::PathBuf;
#[cfg(test)]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::config::Config;

/// Loads and persists the application configuration at one configured path.
#[derive(Clone, Debug)]
pub(crate) struct ConfigRepository {
    path: Option<PathBuf>,
    #[cfg(test)]
    _temp_dir: Option<Arc<tempfile::TempDir>>,
    /// When set, the next [`Self::save`] fails once after load/mutation so
    /// callers can exercise durable-save rollback without OS-specific FS locks.
    /// Shared across clones so the request is not tied to an OS thread.
    #[cfg(test)]
    fail_next_save: Arc<AtomicBool>,
}

impl ConfigRepository {
    pub(crate) fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            #[cfg(test)]
            _temp_dir: None,
            #[cfg(test)]
            fail_next_save: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    pub(crate) fn temporary_for_tests() -> anyhow::Result<Self> {
        let temp_dir = Arc::new(tempfile::tempdir()?);
        Ok(Self {
            path: Some(temp_dir.path().join("config.toml")),
            _temp_dir: Some(temp_dir),
            fail_next_save: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Fail this repository's next `save` once. Shared across clones; not
    /// thread-local, so Tokio worker hops still observe the request.
    #[cfg(test)]
    pub(crate) fn fail_next_save_for_tests(&self) {
        self.fail_next_save.store(true, Ordering::SeqCst);
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

    pub(crate) fn save(&self, config: &Config) -> anyhow::Result<()> {
        #[cfg(test)]
        {
            if self.fail_next_save.swap(false, Ordering::SeqCst) {
                anyhow::bail!("injected config save failure");
            }
        }
        config.save(self.path.clone())
    }

    /// Loads the latest config, applies one typed mutation, and persists it before returning.
    ///
    /// The update is atomic from the caller's perspective, but it does not provide concurrent
    /// filesystem transaction semantics.
    pub(crate) fn update<T>(&self, update: impl FnOnce(&mut Config) -> T) -> anyhow::Result<T> {
        let mut config = self.load()?;
        let value = update(&mut config);
        self.save(&config)?;
        Ok(value)
    }
}

#[cfg(test)]
#[path = "config_repository_tests.rs"]
mod tests;
