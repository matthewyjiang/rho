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
    path::{Component, Path, PathBuf},
};

use crate::{
    diff::unified_diff,
    tool::{truncate, ToolError},
};

use super::{
    parser::{Hunk, UpdateFileChunk},
    seek_sequence::seek_sequence,
};

#[derive(Debug, Clone)]
struct MoveSource {
    path: PathBuf,
    display_path: String,
    content: String,
}

/// One committed filesystem operation. Impossible states are unrepresentable.
#[derive(Debug, Clone)]
enum FileChange {
    Add {
        target: PathBuf,
        display_path: String,
        new_content: String,
        /// Prior contents when the add overwrites an existing file.
        previous_content: Option<String>,
    },
    Delete {
        target: PathBuf,
        display_path: String,
        previous_content: String,
    },
    Update {
        target: PathBuf,
        display_path: String,
        old_content: String,
        new_content: String,
        move_from: Option<MoveSource>,
    },
}

impl FileChange {
    fn summary_line(&self) -> String {
        match self {
            Self::Add { display_path, .. } => format!("A {display_path}"),
            Self::Delete { display_path, .. } => format!("D {display_path}"),
            Self::Update { display_path, .. } => format!("M {display_path}"),
        }
    }

    fn display_path(&self) -> &str {
        match self {
            Self::Add { display_path, .. }
            | Self::Delete { display_path, .. }
            | Self::Update { display_path, .. } => display_path,
        }
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

#[derive(Debug)]
pub(crate) struct ApplyPatchOutcome {
    pub content: String,
    pub display_paths: Vec<String>,
    pub diffs: String,
    pub file_count: usize,
}

pub(crate) async fn apply_hunks(
    hunks: Vec<Hunk>,
    resolve_path: impl Fn(&str) -> Result<PathBuf, ToolError>,
    display_path: impl Fn(&str) -> String,
    max_output_bytes: usize,
) -> Result<ApplyPatchOutcome, ToolError> {
    let mut planned = Vec::with_capacity(hunks.len());
    for hunk in &hunks {
        planned.push(plan_hunk(hunk, &resolve_path, &display_path).await?);
    }
    check_path_conflicts(&planned)?;

    let summary_lines = planned
        .iter()
        .map(FileChange::summary_line)
        .collect::<Vec<_>>();
    let diffs = planned
        .iter()
        .map(FileChange::diff)
        .collect::<Vec<_>>()
        .join("\n\n");
    let display_paths = planned
        .iter()
        .map(|change| change.display_path().to_string())
        .collect::<Vec<_>>();
    let file_count = display_paths.len();

    commit_changes(&planned).await?;

    Ok(ApplyPatchOutcome {
        content: truncate(
            format!(
                "Success. Updated the following files:\n{}\n\n{diffs}",
                summary_lines.join("\n")
            ),
            max_output_bytes,
        ),
        display_paths,
        diffs,
        file_count,
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
            let target = resolve_path(&requested)?;
            let previous_content = read_optional(&target, &display_path(&requested)).await?;
            Ok(FileChange::Add {
                target,
                display_path: display_path(&requested),
                new_content: contents.clone(),
                previous_content,
            })
        }
        Hunk::Delete { path } => {
            let requested = validated_path(path)?;
            let target = resolve_path(&requested)?;
            let previous_content =
                read_required(&target, &display_path(&requested), RequiredRead::Delete).await?;
            Ok(FileChange::Delete {
                target,
                display_path: display_path(&requested),
                previous_content,
            })
        }
        Hunk::Update {
            path,
            move_path,
            chunks,
        } => {
            let requested = validated_path(path)?;
            let source = resolve_path(&requested)?;
            let old_content =
                read_required(&source, &display_path(&requested), RequiredRead::Update).await?;
            let new_content = derive_new_contents(&old_content, &display_path(&requested), chunks)?;
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
                    move_from: Some(MoveSource {
                        path: source,
                        display_path: display_path(&requested),
                        content: old_content,
                    }),
                })
            } else {
                Ok(FileChange::Update {
                    target: source,
                    display_path: display_path(&requested),
                    old_content,
                    new_content,
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

/// Patch paths must stay workspace-relative: no absolute paths and no `..`.
fn validate_patch_path(path: &str) -> Result<(), ToolError> {
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
    let had_trailing_newline = original_contents.ends_with('\n');
    let original_lines = split_lines(original_contents);
    let replacements = compute_replacements(&original_lines, path, chunks)?;
    let new_lines = apply_replacements(original_lines, &replacements);
    Ok(join_lines(&new_lines, had_trailing_newline))
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
            Err(error) => {
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
            previous_content,
            ..
        } => revalidate_optional_snapshot(target, display_path, previous_content).await,
        FileChange::Delete {
            target,
            display_path,
            previous_content,
        } => revalidate_exact_snapshot(target, display_path, previous_content).await,
        FileChange::Update {
            target,
            display_path,
            old_content,
            move_from,
            ..
        } => {
            if let Some(source) = move_from {
                revalidate_exact_snapshot(&source.path, &source.display_path, &source.content)
                    .await?;
                // Move destinations are required to be absent at plan time.
                revalidate_optional_snapshot(target, display_path, &None).await
            } else {
                revalidate_exact_snapshot(target, display_path, old_content).await
            }
        }
    }
}

async fn revalidate_exact_snapshot(
    path: &Path,
    display_path: &str,
    expected: &str,
) -> Result<(), ToolError> {
    let actual = tokio::fs::read_to_string(path).await.map_err(|error| {
        ToolError::Message(format!("Failed to revalidate {display_path}: {error}"))
    })?;
    if actual == expected {
        Ok(())
    } else {
        Err(changed_error(display_path))
    }
}

async fn revalidate_optional_snapshot(
    path: &Path,
    display_path: &str,
    expected: &Option<String>,
) -> Result<(), ToolError> {
    match (expected.as_ref(), tokio::fs::read_to_string(path).await) {
        (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        (Some(expected), Ok(actual)) if actual == *expected => Ok(()),
        (Some(_), Ok(_)) | (None, Ok(_)) => Err(changed_error(display_path)),
        (_, Err(error)) => Err(ToolError::Message(format!(
            "Failed to revalidate {display_path}: {error}"
        ))),
    }
}

fn changed_error(display_path: &str) -> ToolError {
    ToolError::Message(format!(
        "{display_path} changed while the patch was being validated; no files were modified"
    ))
}

async fn apply_one(change: &FileChange) -> Result<(), ToolError> {
    match change {
        FileChange::Add {
            target,
            display_path,
            new_content,
            ..
        } => write_file(target, display_path, new_content).await,
        FileChange::Delete {
            target,
            display_path,
            ..
        } => tokio::fs::remove_file(target).await.map_err(|error| {
            ToolError::Message(format!("failed to delete {display_path}: {error}"))
        }),
        FileChange::Update {
            target,
            display_path,
            new_content,
            move_from,
            ..
        } => {
            write_file(target, display_path, new_content).await?;
            if let Some(source) = move_from {
                if source.path != *target {
                    tokio::fs::remove_file(&source.path)
                        .await
                        .map_err(|error| {
                            ToolError::Message(format!(
                                "failed to remove moved source {}: {error}",
                                source.display_path
                            ))
                        })?;
                }
            }
            Ok(())
        }
    }
}

async fn write_file(path: &Path, display_path: &str, content: &str) -> Result<(), ToolError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            ToolError::Message(format!(
                "failed to create parent directories for {display_path}: {error}"
            ))
        })?;
    }
    tokio::fs::write(path, content)
        .await
        .map_err(|error| ToolError::Message(format!("failed to write {display_path}: {error}")))
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

async fn rollback_one(change: &FileChange) -> Result<(), ToolError> {
    match change {
        FileChange::Add {
            target,
            display_path,
            previous_content,
            ..
        } => match previous_content {
            None => remove_file_if_present(target, display_path).await,
            Some(content) => write_file(target, display_path, content).await,
        },
        FileChange::Delete {
            target,
            display_path,
            previous_content,
        } => write_file(target, display_path, previous_content).await,
        FileChange::Update {
            target,
            display_path,
            old_content,
            move_from,
            ..
        } => {
            if let Some(source) = move_from {
                write_file(&source.path, &source.display_path, &source.content).await?;
                remove_file_if_present(target, display_path).await
            } else {
                write_file(target, display_path, old_content).await
            }
        }
    }
}

async fn remove_file_if_present(path: &Path, display_path: &str) -> Result<(), ToolError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ToolError::Message(format!(
            "failed to remove {display_path}: {error}"
        ))),
    }
}
