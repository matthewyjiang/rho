//! `<rho home>/mcp-serve/watches.json`: the webhook watches the HTTP server
//! keeps, and each webhook's last delivery. The server rewrites it whole at
//! every change, atomically (a temp file, then a rename) and durably (the
//! file flushed before the rename, the directory after), so a restart
//! resumes every watch where it was and `rho mcp webhook list` never reads
//! a torn file. It names sessions and tokens, never a secret; its
//! directory is private.
//!
//! One server keeps it: each takes `watches.lock` beside it for its whole
//! life. A file that does not parse stops the server from starting, naming
//! its path, rather than dropping the watches it holds.
//!
//! The server writes [`Snapshot`]s through a [`Writer`], on the blocking
//! pool rather than an async worker, since a durable write waits on the
//! disk; when two race, the newer one wins.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
};

use anyhow::Context;
use fs2::FileExt;
use rho_protocol::SessionId;
use serde::{Deserialize, Serialize};

use crate::{
    app::session_control::NewsKey,
    config_writer::{write_bytes_atomically_with_durability, WriteDurability},
};

/// The store's directory under the rho home.
pub(crate) const DIR: &str = "mcp-serve";
const FILE: &str = "watches.json";
const LOCK: &str = "watches.lock";

/// Everything the file holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WatchFile {
    #[serde(default)]
    pub(crate) watches: Vec<WatchRecord>,
    /// Each webhook's last delivery, by webhook name.
    #[serde(default)]
    pub(crate) deliveries: BTreeMap<String, DeliveryRecord>,
    /// The `--max-watches` of the server that wrote the file, which `rho
    /// mcp webhook list` shows beside each token's watches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_watches: Option<u32>,
}

/// One watch: token `token` asked to hear of `session` at `webhook`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WatchRecord {
    pub(crate) session: SessionId,
    pub(crate) token: String,
    pub(crate) webhook: String,
    /// The news last delivered, or returned by the `watch` call itself,
    /// which is not delivered again.
    #[serde(default)]
    pub(crate) reported: Option<NewsKey>,
    /// Why the watch stopped short, while it waits for a restart or a
    /// renewal.
    #[serde(default)]
    pub(crate) error: Option<String>,
}

/// A webhook's last delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeliveryRecord {
    /// Unix time, in seconds.
    pub(crate) at: u64,
    pub(crate) session: SessionId,
    pub(crate) result: DeliveryResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryResult {
    /// The receiver took it, with this HTTP status.
    Delivered(u16),
    /// It was given up on: why.
    Failed(String),
}

/// Where the file and its lock are.
#[derive(Clone, Debug)]
pub(crate) struct WatchStore {
    dir: PathBuf,
}

impl WatchStore {
    /// The store in `dir`, `<rho home>/mcp-serve`.
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    pub(crate) fn lock_path(&self) -> PathBuf {
        self.dir.join(LOCK)
    }

    /// Takes the store's lock for as long as the file is held, creating
    /// the private directory first; `None` while another server holds it.
    pub(crate) fn lock(&self) -> anyhow::Result<Option<File>> {
        crate::daemon::create_private_dir(&self.dir)
            .with_context(|| format!("could not create {}", self.dir.display()))?;
        let path = self.lock_path();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("could not open {}", path.display()))?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(file)),
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                Ok(None)
            }
            Err(error) => Err(error).with_context(|| format!("could not lock {}", path.display())),
        }
    }

    /// What the file holds; empty when there is none.
    pub(crate) fn load(&self) -> anyhow::Result<WatchFile> {
        let path = self.path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(WatchFile::default())
            }
            Err(error) => {
                return Err(error).with_context(|| format!("could not read {}", path.display()))
            }
        };
        serde_json::from_str(&text).with_context(|| {
            format!(
                "{} does not parse; fix or remove it to start the server",
                path.display()
            )
        })
    }

    /// Replaces the file with `file`.
    pub(crate) fn save(&self, file: &WatchFile) -> anyhow::Result<()> {
        let path = self.path();
        let mut contents = serde_json::to_vec_pretty(file)?;
        contents.push(b'\n');
        write_bytes_atomically_with_durability(&path, &contents, WriteDurability::Durable)
            .and_then(|()| crate::daemon::sync_dir(&self.dir))
            .with_context(|| format!("could not save {}", path.display()))
    }
}

/// The whole file as the watches were at one moment, numbered in the order
/// the moments came.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) seq: u64,
    pub(super) file: WatchFile,
}

/// Writes snapshots to the store, the newest one winning.
#[derive(Debug)]
pub(super) struct Writer {
    store: WatchStore,
    /// The newest snapshot on disk, held while one is written.
    written: Mutex<u64>,
}

impl Writer {
    pub(super) fn new(store: WatchStore) -> Arc<Self> {
        Arc::new(Self {
            store,
            written: Mutex::new(0),
        })
    }

    /// Writes `snapshot` on the blocking pool, and returns once it is on
    /// disk, or a newer one is.
    pub(super) async fn write(self: &Arc<Self>, snapshot: Snapshot) -> anyhow::Result<()> {
        let writer = Arc::clone(self);
        tokio::task::spawn_blocking(move || writer.write_now(&snapshot))
            .await
            .context("the webhook watch store's writer stopped")?
    }

    /// Writes `snapshot` on this thread, unless a newer one is on disk: it
    /// was taken after this one, so it holds this one's changes too.
    pub(super) fn write_now(&self, snapshot: &Snapshot) -> anyhow::Result<()> {
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        if *written >= snapshot.seq {
            return Ok(());
        }
        self.store.save(&snapshot.file)?;
        *written = snapshot.seq;
        Ok(())
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
