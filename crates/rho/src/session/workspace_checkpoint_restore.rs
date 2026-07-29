use super::*;

impl WorkspaceCheckpointStore {
    pub(crate) fn restore(
        &self,
        checkpoint: &WorkspaceCheckpoint,
        current: &BTreeMap<PathBuf, ObservedFileState>,
        mut observe_fresh: impl FnMut(&Path) -> ObservedFileState,
    ) -> RestoreAudit {
        let entries = checkpoint
            .files
            .iter()
            .map(|file| {
                let current =
                    current
                        .get(&file.path)
                        .cloned()
                        .unwrap_or(ObservedFileState::Unsupported {
                            reason: UnsupportedPath::Unreadable,
                        });
                let classification = classify_restore(file, &current);
                let classification = if matches!(
                    classification,
                    RestoreClassification::Create
                        | RestoreClassification::Modify
                        | RestoreClassification::Delete
                ) && observe_fresh(&file.path) != current
                {
                    RestoreClassification::Conflict
                } else {
                    classification
                };
                let result = match classification {
                    RestoreClassification::Create | RestoreClassification::Modify => {
                        match &file.original {
                            OriginalFileState::Regular(original) => {
                                atomic_restore_file(&file.path, original)
                            }
                            _ => Err(anyhow::anyhow!("checkpoint has no captured file content")),
                        }
                    }
                    RestoreClassification::Delete => remove_captured_file(&file.path),
                    RestoreClassification::Conflict
                    | RestoreClassification::Unsupported
                    | RestoreClassification::Skipped => Ok(()),
                };
                RestoreAuditEntry {
                    path: file.path.clone(),
                    classification,
                    changed: result.is_ok()
                        && matches!(
                            classification,
                            RestoreClassification::Create
                                | RestoreClassification::Modify
                                | RestoreClassification::Delete
                        ),
                    error: result.err().map(|error| error.to_string()),
                }
            })
            .collect();
        RestoreAudit {
            entries,
            limitations: checkpoint.limitations.clone(),
        }
    }
}

/// One pure restore decision for a tracked path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestoreClassification {
    Create,
    Modify,
    Delete,
    Conflict,
    Unsupported,
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestorePlanEntry {
    pub(crate) path: PathBuf,
    pub(crate) classification: RestoreClassification,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestorePlan {
    pub(crate) entries: Vec<RestorePlanEntry>,
    pub(crate) limitations: Vec<UntrackedEffect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestoreAuditEntry {
    pub(crate) path: PathBuf,
    pub(crate) classification: RestoreClassification,
    pub(crate) changed: bool,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestoreAudit {
    pub(crate) entries: Vec<RestoreAuditEntry>,
    pub(crate) limitations: Vec<UntrackedEffect>,
}

fn atomic_restore_file(path: &Path, original: &CapturedRegularFile) -> anyhow::Result<()> {
    static NEXT_TEMP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let parent = path
        .parent()
        .context("checkpoint restore path has no parent directory")?;
    let file_name = path
        .file_name()
        .context("checkpoint restore path has no file name")?
        .to_string_lossy();
    let temp_id = NEXT_TEMP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{file_name}.rho-rewind-{}-{temp_id}",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&temp)?;
        file.write_all(&original.bytes)?;
        file.sync_data()?;
        apply_basic_metadata(&file, &original.metadata)?;
        drop(file);
        replace_file(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn apply_basic_metadata(file: &File, metadata: &BasicFileMetadata) -> std::io::Result<()> {
    #[cfg(unix)]
    if let Some(mode) = metadata.unix_mode {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        return Ok(());
    }
    let mut permissions = file.metadata()?.permissions();
    permissions.set_readonly(metadata.readonly);
    file.set_permissions(permissions)
}

fn remove_captured_file(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "restore target is no longer a regular file"
    );
    fs::remove_file(path)?;
    Ok(())
}

/// Builds a restore preview without reading or writing the workspace.
///
/// `current` must contain a freshly observed state for each authorized checkpoint path.
pub(crate) fn plan_restore(
    checkpoint: &WorkspaceCheckpoint,
    current: &BTreeMap<PathBuf, ObservedFileState>,
) -> anyhow::Result<RestorePlan> {
    let entries = checkpoint
        .files
        .iter()
        .map(|file| {
            let current = current
                .get(&file.path)
                .with_context(|| format!("current state is missing for {}", file.path.display()))?;
            Ok(RestorePlanEntry {
                path: file.path.clone(),
                classification: classify_restore(file, current),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(RestorePlan {
        entries,
        limitations: checkpoint.limitations.clone(),
    })
}

fn classify_restore(file: &FileCheckpoint, current: &ObservedFileState) -> RestoreClassification {
    if matches!(file.original, OriginalFileState::Unsupported { .. })
        || matches!(file.expected_after, ObservedFileState::Unsupported { .. })
    {
        return RestoreClassification::Unsupported;
    }
    if current != &file.expected_after {
        return RestoreClassification::Conflict;
    }

    match (&file.original, &file.expected_after) {
        (OriginalFileState::Absent, ObservedFileState::Regular { .. }) => {
            RestoreClassification::Delete
        }
        (OriginalFileState::Regular(_), ObservedFileState::Absent) => RestoreClassification::Create,
        (OriginalFileState::Regular(original), ObservedFileState::Regular { .. }) => {
            if observed_matches_original(&file.expected_after, original) {
                RestoreClassification::Skipped
            } else {
                RestoreClassification::Modify
            }
        }
        (OriginalFileState::Absent, ObservedFileState::Absent) => RestoreClassification::Skipped,
        (OriginalFileState::Unsupported { .. }, _) | (_, ObservedFileState::Unsupported { .. }) => {
            RestoreClassification::Unsupported
        }
    }
}

fn observed_matches_original(observed: &ObservedFileState, original: &CapturedRegularFile) -> bool {
    matches!(
        observed,
        ObservedFileState::Regular {
            digest,
            size,
            metadata,
        } if digest == &original.digest
            && *size == original.bytes.len() as u64
            && metadata == &original.metadata
    )
}
