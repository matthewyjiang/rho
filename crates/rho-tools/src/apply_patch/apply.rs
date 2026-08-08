//! Apply parsed Codex-style patch hunks to the filesystem.
//!
//! Chunk matching is adapted from the Apache-2.0 codex-rs apply-patch crate.
//!
//! Pipeline:
//! 1. resolve paths and read inputs
//! 2. build a path-keyed [`FileChange`] plan (conflicts fail here)
//! 3. re-read sources immediately before commit and fail closed if they changed
//! 4. commit in patch order with enum-driven rollback on failure

use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path, PathBuf},
};

use crate::{
    diff::unified_diff,
    file_mutation::{
        atomic_create_file, lock_for_rewrite, normalize_newlines, preferred_line_ending,
        rewrite_locked_file, FileMutationOutcome,
    },
    tool::{truncate, ToolError},
};

use super::{
    parser::{Hunk, UpdateFileChunk},
    seek_sequence::seek_sequence,
};

#[derive(Debug, Clone)]
pub(super) struct MoveSource {
    path: PathBuf,
    display_path: String,
    content: String,
}

/// One committed filesystem operation. Impossible states are unrepresentable.
#[derive(Debug, Clone)]
pub(super) enum FileChange {
    Add {
        target: PathBuf,
        display_path: String,
        new_content: String,
    },
    Delete {
        target: PathBuf,
        display_path: String,
        previous_content: String,
        previous_permissions: std::fs::Permissions,
    },
    Update {
        target: PathBuf,
        display_path: String,
        old_content: String,
        new_content: String,
        permissions: std::fs::Permissions,
        move_from: Option<MoveSource>,
    },
}

struct ApplyFailure {
    error: ToolError,
    mutation_started: bool,
}

impl ApplyFailure {
    fn before_mutation(error: ToolError) -> Self {
        Self {
            error,
            mutation_started: false,
        }
    }

    fn after_mutation(error: ToolError) -> Self {
        Self {
            error,
            mutation_started: true,
        }
    }
}

impl FileChange {
    fn summary_line(&self) -> String {
        match self {
            Self::Add { display_path, .. } => format!("A {display_path}"),
            Self::Delete { display_path, .. } => format!("D {display_path}"),
            Self::Update { display_path, .. } => format!("M {display_path}"),
        }
    }

    fn affected_display_paths(&self) -> impl Iterator<Item = &str> {
        let paths = match self {
            Self::Update {
                display_path,
                move_from: Some(source),
                ..
            } => [
                Some(source.display_path.as_str()),
                Some(display_path.as_str()),
            ],
            Self::Add { display_path, .. }
            | Self::Delete { display_path, .. }
            | Self::Update { display_path, .. } => [Some(display_path.as_str()), None],
        };
        paths.into_iter().flatten()
    }

    fn diff(&self) -> String {
        match self {
            Self::Add {
                display_path,
                new_content,
                ..
            } => unified_diff("", new_content, display_path, /*created*/ true),
            Self::Delete {
                display_path,
                previous_content,
                ..
            } => unified_diff(previous_content, "", display_path, /*created*/ false),
            Self::Update {
                display_path,
                old_content,
                new_content,
                ..
            } => unified_diff(
                old_content,
                new_content,
                display_path,
                /*created*/ false,
            ),
        }
    }

    fn chain_snapshot(&self) -> Option<String> {
        match self {
            Self::Add {
                display_path,
                new_content,
                ..
            }
            | Self::Update {
                display_path,
                new_content,
                ..
            } => Some(crate::hashline::format_chain_snapshot(
                display_path,
                new_content,
                &[],
            )),
            Self::Delete { .. } => None,
        }
    }

    fn write_target(&self) -> Option<(&PathBuf, &str)> {
        match self {
            Self::Add {
                target,
                display_path,
                ..
            }
            | Self::Update {
                target,
                display_path,
                ..
            } => Some((target, display_path)),
            Self::Delete { .. } => None,
        }
    }

    fn delete_target(&self) -> Option<(&PathBuf, &str)> {
        match self {
            Self::Delete {
                target,
                display_path,
                ..
            } => Some((target, display_path)),
            Self::Update {
                move_from: Some(source),
                ..
            } => Some((&source.path, &source.display_path)),
            Self::Add { .. }
            | Self::Update {
                move_from: None, ..
            } => None,
        }
    }
}

pub(crate) async fn apply_hunks(
    hunks: Vec<Hunk>,
    resolve_path: impl Fn(&str) -> Result<PathBuf, ToolError>,
    display_path: impl Fn(&str) -> String,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError> {
    let mut planned = Vec::with_capacity(hunks.len());
    for hunk in &hunks {
        planned.push(plan_hunk(hunk, &resolve_path, &display_path).await?);
    }
    check_path_conflicts(&planned)?;

    let summary_lines = planned
        .iter()
        .map(FileChange::summary_line)
        .collect::<Vec<_>>();
    let diff = planned
        .iter()
        .map(FileChange::diff)
        .collect::<Vec<_>>()
        .join("\n\n");
    let display_paths = planned
        .iter()
        .flat_map(FileChange::affected_display_paths)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let snapshots = planned
        .iter()
        .filter_map(FileChange::chain_snapshot)
        .collect::<Vec<_>>();

    commit_changes(&planned).await?;

    let mut content = format!(
        "Success. Updated the following files:\n{}",
        summary_lines.join("\n")
    );
    if !snapshots.is_empty() {
        content.push_str("\n\n");
        content.push_str(&snapshots.join("\n\n"));
    }
    Ok(FileMutationOutcome {
        content: truncate(content, max_output_bytes),
        display_paths,
        diff,
    })
}

/// Fail when two ops write the same path, delete the same path, or both write and
/// delete the same path (for example delete A + move A → B).
fn check_path_conflicts(changes: &[FileChange]) -> Result<(), ToolError> {
    let mut writes = BTreeMap::<PathBuf, String>::new();
    let mut deletes = BTreeMap::<PathBuf, String>::new();

    for change in changes {
        if let Some((path, display)) = change.write_target() {
            if let Some(previous) = writes.insert(path.clone(), display.to_string()) {
                return Err(ToolError::Message(format!(
                    "patch targets '{display}' more than once (also as '{previous}')"
                )));
            }
        }
        if let Some((path, display)) = change.delete_target() {
            if let Some(previous) = deletes.insert(path.clone(), display.to_string()) {
                return Err(ToolError::Message(format!(
                    "patch deletes '{display}' more than once (also as '{previous}')"
                )));
            }
        }
    }

    for (path, write_display) in &writes {
        if let Some(delete_display) = deletes.get(path) {
            return Err(ToolError::Message(format!(
                "patch both writes and deletes '{write_display}' (also deleted as '{delete_display}')"
            )));
        }
    }
    Ok(())
}

async fn plan_hunk(
    hunk: &Hunk,
    resolve_path: &impl Fn(&str) -> Result<PathBuf, ToolError>,
    display_path: &impl Fn(&str) -> String,
) -> Result<FileChange, ToolError> {
    match hunk {
        Hunk::Add { path, contents } => {
            let requested = validated_path(path)?;
            let display = display_path(&requested);
            let target = resolve_path(&requested)?;
            if read_optional(&target, &display).await?.is_some() {
                return Err(ToolError::Message(format!(
                    "Refusing to add '{display}': file already exists"
                )));
            }
            Ok(FileChange::Add {
                target,
                display_path: display,
                new_content: contents.clone(),
            })
        }
        Hunk::Delete { path } => {
            let requested = validated_path(path)?;
            let display = display_path(&requested);
            let target = resolve_path(&requested)?;
            reject_symlink_entry(&target, &display)?;
            let previous_permissions = read_permissions(&target, &display).await?;
            let previous_content = read_required(&target, &display, RequiredRead::Delete).await?;
            Ok(FileChange::Delete {
                target,
                display_path: display,
                previous_content,
                previous_permissions,
            })
        }
        Hunk::Update {
            path,
            move_path,
            chunks,
        } => {
            let requested = validated_path(path)?;
            let source_display = display_path(&requested);
            let source = resolve_path(&requested)?;
            if move_path.is_some() {
                reject_symlink_entry(&source, &source_display)?;
            }
            let permissions = read_permissions(&source, &source_display).await?;
            let old_content = read_required(&source, &source_display, RequiredRead::Update).await?;
            let new_content = derive_new_contents(&old_content, &source_display, chunks)?;
            if let Some(dest) = move_path {
                let dest_requested = validated_path(dest)?;
                let target = resolve_path(&dest_requested)?;
                let dest_display = display_path(&dest_requested);
                // Moves must not silently clobber an existing destination.
                if read_optional(&target, &dest_display).await?.is_some() {
                    return Err(ToolError::Message(format!(
                        "Refusing to move to '{dest_display}': destination already exists"
                    )));
                }
                Ok(FileChange::Update {
                    target,
                    display_path: dest_display,
                    old_content: old_content.clone(),
                    new_content,
                    permissions: permissions.clone(),
                    move_from: Some(MoveSource {
                        path: source,
                        display_path: source_display,
                        content: old_content,
                    }),
                })
            } else {
                Ok(FileChange::Update {
                    target: source,
                    display_path: source_display,
                    old_content,
                    new_content,
                    permissions,
                    move_from: None,
                })
            }
        }
    }
}

fn validated_path(path: &str) -> Result<String, ToolError> {
    validate_patch_path(path)?;
    Ok(path.to_string())
}

pub(crate) fn validate_hunk_paths(hunk: &Hunk) -> Result<(), ToolError> {
    validate_patch_path(hunk.source_path())?;
    if let Some(destination) = hunk.move_destination() {
        validate_patch_path(destination)?;
    }
    Ok(())
}

/// Patch paths must stay workspace-relative: no absolute paths and no `..`.
pub(crate) fn validate_patch_path(path: &str) -> Result<(), ToolError> {
    let candidate = Path::new(path);
    // Reject Unix-root and Windows-prefix forms even when `is_absolute` is false
    // (for example `/tmp/x` on Windows).
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
    {
        return Err(ToolError::Message(format!(
            "patch path must be relative: {path}"
        )));
    }
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ToolError::Message(format!(
            "patch path must not contain '..': {path}"
        )));
    }
    Ok(())
}

pub(crate) fn reject_symlink_entry(path: &Path, display: &str) -> Result<(), ToolError> {
    if std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(ToolError::Message(format!(
            "apply_patch cannot delete or move symlink path '{display}'"
        )));
    }
    Ok(())
}
async fn read_optional(path: &Path, display: &str) -> Result<Option<String>, ToolError> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ToolError::Message(format!(
            "Failed to read file {display}: {error}"
        ))),
    }
}

#[derive(Debug, Clone, Copy)]
enum RequiredRead {
    Delete,
    Update,
}

async fn read_permissions(path: &Path, display: &str) -> Result<std::fs::Permissions, ToolError> {
    tokio::fs::metadata(path)
        .await
        .map(|metadata| metadata.permissions())
        .map_err(|error| {
            ToolError::Message(format!("Failed to read permissions for {display}: {error}"))
        })
}

async fn read_required(
    path: &Path,
    display: &str,
    action: RequiredRead,
) -> Result<String, ToolError> {
    tokio::fs::read_to_string(path).await.map_err(|error| {
        let verb = match action {
            RequiredRead::Delete => "Failed to delete file",
            RequiredRead::Update => "Failed to read file to update",
        };
        ToolError::Message(format!("{verb} {display}: {error}"))
    })
}

fn derive_new_contents(
    original_contents: &str,
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<String, ToolError> {
    let line_ending = preferred_line_ending(original_contents);
    let normalized_original = normalize_newlines(original_contents);
    let had_trailing_newline = normalized_original.ends_with('\n');
    let original_lines = split_lines(&normalized_original);
    let replacements = compute_replacements(&original_lines, path, chunks)?;
    let new_lines = apply_replacements(original_lines, &replacements);
    Ok(join_lines(&new_lines, had_trailing_newline).replace('\n', line_ending))
}

fn split_lines(contents: &str) -> Vec<String> {
    if contents.is_empty() {
        return Vec::new();
    }
    let body = contents.strip_suffix('\n').unwrap_or(contents);
    if body.is_empty() {
        // File was a single trailing newline.
        return vec![String::new()];
    }
    body.split('\n').map(String::from).collect()
}

fn join_lines(lines: &[String], trailing_newline: bool) -> String {
    let mut out = lines.join("\n");
    if trailing_newline {
        out.push('\n');
    }
    out
}

fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, ToolError> {
    let mut replacements = Vec::new();
    let mut line_index = 0usize;
    let mut min_next_start = 0usize;

    for chunk in chunks {
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                /*eof*/ false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(ToolError::Message(format!(
                    "Failed to find context '{ctx_line}' in {path}"
                )));
            }
        }

        if chunk.old_lines.is_empty() {
            // Pure addition inserts at the current context cursor, not EOF.
            // Keep line_index as an index into original_lines so later chunks
            // still search the original file coordinates.
            if line_index < min_next_start {
                return Err(ToolError::Message(format!(
                    "patch chunks overlap or apply out of order in {path}"
                )));
            }
            replacements.push((line_index, 0, chunk.new_lines.clone()));
            min_next_start = line_index;
            continue;
        }

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        if let Some(start_idx) = found {
            if start_idx < min_next_start {
                return Err(ToolError::Message(format!(
                    "patch chunks overlap or apply out of order in {path}"
                )));
            }
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            min_next_start = start_idx + pattern.len();
            line_index = min_next_start;
        } else {
            return Err(ToolError::Message(format!(
                "Failed to find expected lines in {path}:\n{}",
                chunk.old_lines.join("\n")
            )));
        }
    }

    Ok(replacements)
}

fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        let old_len = *old_len;
        let end = (start_idx + old_len).min(lines.len());
        if start_idx <= lines.len() {
            lines.splice(start_idx..end, new_segment.iter().cloned());
        }
    }
    lines
}

async fn commit_changes(planned: &[FileChange]) -> Result<(), ToolError> {
    // Fail closed if the workspace changed after planning.
    for change in planned {
        revalidate_change(change).await?;
    }

    let mut applied = Vec::new();
    for change in planned {
        match apply_one(change).await {
            Ok(()) => applied.push(change),
            Err(ApplyFailure {
                error,
                mutation_started,
            }) => {
                if mutation_started {
                    applied.push(change);
                }
                let mut message = error.to_string();
                if let Err(rollback_error) = rollback_applied(&applied).await {
                    message = format!("{message}; rollback also failed: {rollback_error}");
                } else if !applied.is_empty() {
                    message = format!("{message}; applied changes were rolled back");
                }
                return Err(ToolError::Message(message));
            }
        }
    }
    Ok(())
}

async fn revalidate_change(change: &FileChange) -> Result<(), ToolError> {
    match change {
        FileChange::Add {
            target,
            display_path,
            ..
        } => expect_live(target, display_path, None).await,
        FileChange::Delete {
            target,
            display_path,
            previous_content,
            ..
        } => expect_live(target, display_path, Some(previous_content)).await,
        FileChange::Update {
            target,
            display_path,
            old_content,
            move_from,
            ..
        } => {
            if let Some(source) = move_from {
                expect_live(&source.path, &source.display_path, Some(&source.content)).await?;
                expect_live(target, display_path, None).await
            } else {
                expect_live(target, display_path, Some(old_content)).await
            }
        }
    }
}

async fn expect_live(
    path: &Path,
    display_path: &str,
    expected: Option<&str>,
) -> Result<(), ToolError> {
    let live = read_live(path, display_path).await?;
    let matches = match (expected, &live) {
        (None, LiveText::Missing) => true,
        (Some(expected), LiveText::Content(actual)) => expected == actual,
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(changed_error(display_path))
    }
}

fn changed_error(display_path: &str) -> ToolError {
    ToolError::Message(format!(
        "{display_path} changed while the patch was being validated; no files were modified"
    ))
}

async fn rewrite_current(
    path: &Path,
    display_path: &str,
    expected: &str,
    updated: &str,
) -> Result<(), ToolError> {
    let path = path.to_path_buf();
    let display_path = display_path.to_string();
    let expected = expected.to_string();
    let updated = updated.to_string();
    tokio::task::spawn_blocking(move || {
        let mut file = lock_for_rewrite(&path, &display_path, " for apply_patch")?;
        ensure_locked_path_identity(&file, &path, &display_path)?;
        let mut live = String::new();
        file.read_to_string(&mut live).map_err(|error| {
            ToolError::Message(format!("failed to re-read {display_path}: {error}"))
        })?;
        if live != expected {
            return Err(changed_error(&display_path));
        }
        rewrite_locked_file(&mut file, &display_path, &live, &updated)
    })
    .await
    .map_err(|error| ToolError::Message(format!("apply_patch task failed: {error}")))?
}

async fn remove_current(path: &Path, display_path: &str, expected: &str) -> Result<(), ToolError> {
    let path = path.to_path_buf();
    let display_path = display_path.to_string();
    let expected = expected.to_string();
    tokio::task::spawn_blocking(move || {
        let mut file = lock_for_rewrite(&path, &display_path, " for apply_patch")?;
        ensure_locked_path_identity(&file, &path, &display_path)?;
        let mut live = String::new();
        file.read_to_string(&mut live).map_err(|error| {
            ToolError::Message(format!("failed to re-read {display_path}: {error}"))
        })?;
        if live != expected {
            return Err(changed_error(&display_path));
        }
        std::fs::remove_file(&path).map_err(|error| {
            ToolError::Message(format!("failed to delete {display_path}: {error}"))
        })
    })
    .await
    .map_err(|error| ToolError::Message(format!("apply_patch task failed: {error}")))?
}

fn ensure_locked_path_identity(
    file: &std::fs::File,
    path: &Path,
    display_path: &str,
) -> Result<(), ToolError> {
    let live = std::fs::symlink_metadata(path).map_err(|error| {
        ToolError::Message(format!("failed to inspect live {display_path}: {error}"))
    })?;
    let same_file = same_file_identity(file, path).map_err(|error| {
        ToolError::Message(format!("failed to compare live {display_path}: {error}"))
    })?;
    if live.file_type().is_symlink() || !same_file {
        return Err(changed_error(display_path));
    }
    Ok(())
}

#[cfg(unix)]
fn same_file_identity(file: &std::fs::File, path: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let left = file.metadata()?;
    let right = std::fs::metadata(path)?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(windows)]
fn same_file_identity(file: &std::fs::File, path: &Path) -> std::io::Result<bool> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};

    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    fn identity(file: &std::fs::File) -> std::io::Result<(u32, u64)> {
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        // SAFETY: `file` owns a valid handle and `info` points to writable storage.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: A successful call initialized the full structure.
        let info = unsafe { info.assume_init() };
        let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        Ok((info.dwVolumeSerialNumber, index))
    }

    let live = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    Ok(identity(file)? == identity(&live)?)
}

async fn apply_one(change: &FileChange) -> Result<(), ApplyFailure> {
    match change {
        FileChange::Add {
            target,
            display_path,
            new_content,
        } => atomic_create_file(target, display_path, new_content, None)
            .await
            .map_err(ApplyFailure::before_mutation),
        FileChange::Delete {
            target,
            display_path,
            previous_content,
            ..
        } => remove_current(target, display_path, previous_content)
            .await
            .map_err(ApplyFailure::before_mutation),
        FileChange::Update {
            target,
            display_path,
            old_content,
            new_content,
            permissions,
            move_from,
            ..
        } => {
            if let Some(source) = move_from {
                atomic_create_file(target, display_path, new_content, Some(permissions.clone()))
                    .await
                    .map_err(ApplyFailure::before_mutation)?;
                if source.path != *target {
                    remove_current(&source.path, &source.display_path, &source.content)
                        .await
                        .map_err(ApplyFailure::after_mutation)?;
                }
                Ok(())
            } else {
                rewrite_current(target, display_path, old_content, new_content)
                    .await
                    .map_err(ApplyFailure::before_mutation)
            }
        }
    }
}

async fn rollback_applied(applied: &[&FileChange]) -> Result<(), ToolError> {
    let mut failures = Vec::new();
    for change in applied.iter().rev() {
        if let Err(error) = rollback_one(change).await {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ToolError::Message(failures.join("; ")))
    }
}

pub(super) async fn rollback_one(change: &FileChange) -> Result<(), ToolError> {
    match change {
        FileChange::Add {
            target,
            display_path,
            new_content,
        } => match read_live(target, display_path).await? {
            LiveText::Missing => Ok(()),
            LiveText::Content(live) if live == *new_content => {
                remove_current(target, display_path, new_content).await
            }
            LiveText::Content(_) => Err(concurrent_rollback_error(display_path)),
        },
        FileChange::Delete {
            target,
            display_path,
            previous_content,
            previous_permissions,
        } => match read_live(target, display_path).await? {
            LiveText::Missing => {
                atomic_create_file(
                    target,
                    display_path,
                    previous_content,
                    Some(previous_permissions.clone()),
                )
                .await
            }
            LiveText::Content(live) if live == *previous_content => Ok(()),
            LiveText::Content(_) => Err(concurrent_rollback_error(display_path)),
        },
        FileChange::Update {
            target,
            display_path,
            old_content,
            new_content,
            permissions,
            move_from,
        } => {
            if let Some(source) = move_from {
                rollback_move(
                    source,
                    target,
                    display_path,
                    new_content,
                    permissions.clone(),
                )
                .await
            } else {
                match read_live(target, display_path).await? {
                    LiveText::Content(live) if live == *new_content => {
                        rewrite_current(target, display_path, new_content, old_content).await
                    }
                    LiveText::Content(live) if live == *old_content => Ok(()),
                    LiveText::Missing | LiveText::Content(_) => {
                        Err(concurrent_rollback_error(display_path))
                    }
                }
            }
        }
    }
}

async fn rollback_move(
    source: &MoveSource,
    target: &Path,
    target_display: &str,
    new_content: &str,
    permissions: std::fs::Permissions,
) -> Result<(), ToolError> {
    let source_live = read_live(&source.path, &source.display_path).await?;
    let target_live = read_live(target, target_display).await?;
    let source_unchanged = match &source_live {
        LiveText::Missing => true,
        LiveText::Content(live) => live == &source.content,
    };
    if !source_unchanged {
        return Err(concurrent_rollback_error(&source.display_path));
    }
    let target_unchanged = match &target_live {
        LiveText::Missing => true,
        LiveText::Content(live) => live == new_content,
    };
    if !target_unchanged {
        return Err(concurrent_rollback_error(target_display));
    }

    if matches!(source_live, LiveText::Missing) {
        atomic_create_file(
            &source.path,
            &source.display_path,
            &source.content,
            Some(permissions),
        )
        .await?;
    }
    if matches!(target_live, LiveText::Content(_)) {
        remove_current(target, target_display, new_content).await?;
    }
    Ok(())
}

enum LiveText {
    Missing,
    Content(String),
}

async fn read_live(path: &Path, display_path: &str) -> Result<LiveText, ToolError> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(LiveText::Content(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LiveText::Missing),
        Err(error) => Err(ToolError::Message(format!(
            "failed to inspect {display_path} for rollback: {error}"
        ))),
    }
}

fn concurrent_rollback_error(display_path: &str) -> ToolError {
    ToolError::Message(format!(
        "{display_path}: changed after apply; refusing rollback to avoid clobbering concurrent writers"
    ))
}
