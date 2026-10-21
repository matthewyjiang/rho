//! Text files an embedding host serves in place of local disk.
//!
//! An editor host holds unsaved buffers and tracks the agent's edits. When it
//! supplies [`HostTextFiles`], text tools read a served path's current text
//! from the host and hand their result back to it, instead of touching the
//! file on disk. Everything else stays with the tools: path resolution and
//! authorization, file kind and size checks on local disk, edit matching,
//! diffs, and output formatting.
//!
//! One path always takes one route, so the text a read shows is the text a
//! later edit of that path validates against.

use std::{
    collections::HashMap,
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Weak},
};

use crate::{
    document::MAX_DOCUMENT_INPUT_BYTES,
    file_mutation::{lock_for_rewrite, locked_path_identity_matches, rewrite_locked_file},
    tool::ToolError,
};

/// Future returned by host operations, such as [`HostTextFiles`] reads.
pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, HostError>> + Send + 'a>>;

/// UTF-8 text files served by the embedding host, such as an editor.
///
/// Rho resolves and authorizes every path before calling an implementor, and
/// passes it absolute. Implementors only move text. Dropping a returned future
/// abandons the operation; a write abandoned that way may still have applied.
pub trait HostTextFiles: Send + Sync {
    /// Whether the host serves `path`. Rho reads and writes every other path
    /// on local disk. The answer must not change for a path during a session.
    fn serves(&self, path: &Path) -> bool;

    /// The host's current text of an existing file, including unsaved edits.
    fn read_text<'a>(&'a self, path: &'a Path) -> HostFuture<'a, String>;

    /// Replaces the file's text, creating the file when it does not exist.
    /// The host may transform the text as it saves, for example by
    /// formatting it; Rho reads the file back afterwards.
    fn write_text<'a>(&'a self, path: &'a Path, text: &'a str) -> HostFuture<'a, ()>;
}

/// Why a host operation failed, in the host's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostError(String);

impl HostError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HostError {}

/// Where one path's text lives. Owned, so a multi-file commit can outlive
/// the tool call that started it.
#[derive(Clone)]
pub(crate) enum TextRoute {
    Disk,
    Host(Arc<dyn HostTextFiles>),
}

impl std::fmt::Debug for TextRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Disk => "Disk",
            Self::Host(_) => "Host",
        })
    }
}

/// Why a rewrite was not committed.
pub(crate) enum CommitError {
    /// The path no longer names the file that was read.
    PathChanged,
    /// The file's text changed after it was read; `live` is the current text.
    Changed {
        live: String,
    },
    Failed(ToolError),
}

impl TextRoute {
    pub(crate) fn for_path(host: Option<&Arc<dyn HostTextFiles>>, path: &Path) -> Self {
        match host {
            Some(host) if host.serves(path) => Self::Host(Arc::clone(host)),
            _ => Self::Disk,
        }
    }

    pub(crate) fn is_host(&self) -> bool {
        matches!(self, Self::Host(_))
    }

    /// Reads an existing file's text. The host's text is held to the same
    /// input limit as a document on disk, since its buffer can outgrow the
    /// file whose size was checked.
    pub(crate) async fn read(&self, path: &Path, display_path: &str) -> Result<String, ToolError> {
        match self {
            Self::Disk => tokio::fs::read_to_string(path).await.map_err(|error| {
                ToolError::Message(format!("could not read {display_path}: {error}"))
            }),
            Self::Host(host) => {
                within_input_limit(read_host(host, path, display_path).await?, display_path)
            }
        }
    }

    /// The host's text of a file missing on disk, or `None` when the host
    /// holds none either. Hosts report a missing file in their own words, so
    /// any failed read means none; text the host returns counts even past the
    /// input limit, which then fails the call instead of hiding the file.
    pub(crate) async fn read_if_held(
        &self,
        path: &Path,
        display_path: &str,
    ) -> Result<Option<String>, ToolError> {
        match self {
            Self::Disk => Ok(None),
            Self::Host(host) => match host.read_text(path).await {
                Ok(text) => within_input_limit(text, display_path).map(Some),
                Err(_) => Ok(None),
            },
        }
    }

    /// Writes a whole file, creating it when missing, and returns the text
    /// the file then holds. Parent directories must already exist.
    pub(crate) async fn write(
        &self,
        path: &Path,
        display_path: &str,
        text: &str,
    ) -> Result<String, ToolError> {
        match self {
            Self::Disk => {
                tokio::fs::write(path, text).await?;
                Ok(text.to_string())
            }
            Self::Host(host) => {
                let _held = host_path_lock(path).lock_owned().await;
                write_host(host, path, display_path, text).await
            }
        }
    }

    /// Replaces `original` with `updated`, refusing when the file no longer
    /// holds `original`, and returns the text the file then holds (see
    /// [`Self::write`]). On disk the check and rewrite run under an exclusive
    /// lock. The host applies its write to its own copy, so there the check
    /// is a fresh read just before writing, under a lock Rho holds per path.
    pub(crate) async fn commit(
        &self,
        path: &Path,
        display_path: &str,
        original: &str,
        updated: &str,
    ) -> Result<String, CommitError> {
        match self {
            Self::Disk => {
                let (path, display_path) = (path.to_path_buf(), display_path.to_string());
                let (original, updated) = (original.to_string(), updated.to_string());
                tokio::task::spawn_blocking(move || {
                    commit_on_disk(&path, &display_path, &original, &updated)
                })
                .await
                .map_err(|error| {
                    CommitError::Failed(ToolError::Message(format!("edit task failed: {error}")))
                })?
            }
            Self::Host(host) => {
                let _held = host_path_lock(path).lock_owned().await;
                // Not held to the input limit: this checks text Rho already
                // holds, such as an edit it is rolling back.
                let live = read_host(host, path, display_path)
                    .await
                    .map_err(CommitError::Failed)?;
                if live != original {
                    return Err(CommitError::Changed { live });
                }
                write_host(host, path, display_path, updated)
                    .await
                    .map_err(CommitError::Failed)
            }
        }
    }
}

async fn read_host(
    host: &Arc<dyn HostTextFiles>,
    path: &Path,
    display_path: &str,
) -> Result<String, ToolError> {
    host.read_text(path).await.map_err(|error| {
        ToolError::Message(format!(
            "could not read {display_path} from the host: {error}"
        ))
    })
}

fn within_input_limit(text: String, display_path: &str) -> Result<String, ToolError> {
    if text.len() > MAX_DOCUMENT_INPUT_BYTES {
        return Err(ToolError::Message(format!(
            "the host's text of {display_path} is {} bytes; the input limit is {MAX_DOCUMENT_INPUT_BYTES} bytes",
            text.len()
        )));
    }
    Ok(text)
}

/// Writes through the host and returns the text the file then holds.
async fn write_host(
    host: &Arc<dyn HostTextFiles>,
    path: &Path,
    display_path: &str,
    text: &str,
) -> Result<String, ToolError> {
    host.write_text(path, text).await.map_err(|error| {
        ToolError::Message(format!(
            "could not write {display_path} through the host: {error}"
        ))
    })?;
    // The host may have formatted the text as it saved. Diffs, snapshots,
    // and rollback need the text the file holds. The write already applied,
    // so a failed read-back is no reason to fail the call; the text sent is
    // the best estimate.
    Ok(host
        .read_text(path)
        .await
        .unwrap_or_else(|_| text.to_string()))
}

/// The lock that serializes Rho's host writes to `path`, as the file lock
/// does on disk. A multi-file commit settles even after its call is
/// cancelled, so a later call's commit must wait for it rather than
/// interleave. Process-wide, like the file lock it stands in for.
fn host_path_lock(path: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::LazyLock<
        std::sync::Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>,
    > = std::sync::LazyLock::new(Default::default);
    let mut locks = LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(lock) = locks.get(path).and_then(Weak::upgrade) {
        return lock;
    }
    locks.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

fn commit_on_disk(
    path: &Path,
    display_path: &str,
    original: &str,
    updated: &str,
) -> Result<String, CommitError> {
    use std::io::Read;

    let mut file = lock_for_rewrite(path, display_path, "").map_err(CommitError::Failed)?;
    if !locked_path_identity_matches(&file, path, display_path).map_err(CommitError::Failed)? {
        return Err(CommitError::PathChanged);
    }
    let mut live = String::new();
    file.read_to_string(&mut live).map_err(|error| {
        CommitError::Failed(ToolError::Message(format!(
            "could not read {display_path}: {error}"
        )))
    })?;
    if live != original {
        return Err(CommitError::Changed { live });
    }
    rewrite_locked_file(&mut file, display_path, original, updated).map_err(CommitError::Failed)?;
    Ok(updated.to_string())
}

#[cfg(test)]
#[path = "host_files_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) mod test_support {
    //! An in-memory host for tool tests.

    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };

    use tokio::sync::{watch, Notify};

    use super::{HostError, HostFuture, HostTextFiles};

    /// Serves every path under `root` from memory and records each write.
    /// Like an editor that formats on save, it stores `format(text)` for a
    /// write of `text`; it holds writes to `hold` until released, then
    /// refuses writes to `refused`. `applied` counts the writes it stored.
    pub(crate) struct MemoryHost {
        pub(crate) root: PathBuf,
        pub(crate) files: Mutex<BTreeMap<PathBuf, String>>,
        pub(crate) writes: Mutex<Vec<(PathBuf, String)>>,
        pub(crate) format: fn(&str) -> String,
        pub(crate) refused: Option<PathBuf>,
        pub(crate) hold: Option<Hold>,
        pub(crate) applied: watch::Sender<usize>,
    }

    /// Holds the first write to `path`: `held` is notified when it arrives,
    /// and it then waits for `release`. Later writes pass.
    pub(crate) struct Hold {
        pub(crate) path: PathBuf,
        pub(crate) held: Arc<Notify>,
        pub(crate) release: Arc<Notify>,
        pub(crate) used: std::sync::atomic::AtomicBool,
    }

    impl MemoryHost {
        pub(crate) fn new(root: &Path, files: &[(&Path, &str)]) -> Self {
            Self {
                root: root.to_path_buf(),
                files: Mutex::new(
                    files
                        .iter()
                        .map(|(path, text)| (path.to_path_buf(), (*text).to_string()))
                        .collect(),
                ),
                writes: Mutex::default(),
                format: str::to_string,
                refused: None,
                hold: None,
                applied: watch::Sender::new(0),
            }
        }
    }

    impl HostTextFiles for MemoryHost {
        fn serves(&self, path: &Path) -> bool {
            path.starts_with(&self.root)
        }

        fn read_text<'a>(&'a self, path: &'a Path) -> HostFuture<'a, String> {
            Box::pin(async move {
                self.files
                    .lock()
                    .unwrap()
                    .get(path)
                    .cloned()
                    .ok_or_else(|| HostError::new("not open"))
            })
        }

        fn write_text<'a>(&'a self, path: &'a Path, text: &'a str) -> HostFuture<'a, ()> {
            Box::pin(async move {
                if let Some(hold) = self.hold.as_ref().filter(|hold| {
                    hold.path == path && !hold.used.swap(true, std::sync::atomic::Ordering::SeqCst)
                }) {
                    hold.held.notify_one();
                    hold.release.notified().await;
                }
                if self.refused.as_deref() == Some(path) {
                    return Err(HostError::new("refused"));
                }
                self.files
                    .lock()
                    .unwrap()
                    .insert(path.to_path_buf(), (self.format)(text));
                self.writes
                    .lock()
                    .unwrap()
                    .push((path.to_path_buf(), text.to_string()));
                self.applied.send_modify(|count| *count += 1);
                Ok(())
            })
        }
    }
}
