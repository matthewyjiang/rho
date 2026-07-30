//! Apply parsed Codex-style patch hunks to the filesystem.
//!
//! Chunk matching is adapted from the Apache-2.0 codex-rs apply-patch crate.
//! Writes validate fully, then commit with best-effort rollback.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeKind {
    Add,
    Delete,
    Update,
}

struct PlannedChange {
    kind: ChangeKind,
    /// Path written or deleted by this change.
    target: PathBuf,
    display_path: String,
    /// Optional source path removed after a move.
    remove_source: Option<PathBuf>,
    remove_source_display: Option<String>,
    original_target: Option<String>,
    original_source: Option<String>,
    new_content: Option<String>,
}

impl PlannedChange {
    fn source_display(&self) -> String {
        self.remove_source_display
            .clone()
            .or_else(|| {
                self.remove_source
                    .as_ref()
                    .map(|path| path.display().to_string())
            })
            .unwrap_or_else(|| self.display_path.clone())
    }
}

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

    // Detect overlapping write targets before mutating the workspace.
    let mut seen_targets = BTreeMap::<PathBuf, String>::new();
    for change in &planned {
        if let Some(previous) =
            seen_targets.insert(change.target.clone(), change.display_path.clone())
        {
            return Err(ToolError::Message(format!(
                "patch targets '{}' more than once (also as '{previous}')",
                change.display_path
            )));
        }
    }
    for change in &planned {
        if let Some(source) = &change.remove_source {
            if seen_targets.contains_key(source) {
                return Err(ToolError::Message(format!(
                    "patch both writes and deletes '{}'",
                    change.source_display()
                )));
            }
        }
    }

    let summary_lines = planned
        .iter()
        .map(|change| {
            let prefix = match change.kind {
                ChangeKind::Add => 'A',
                ChangeKind::Delete => 'D',
                ChangeKind::Update => 'M',
            };
            format!("{prefix} {}", change.display_path)
        })
        .collect::<Vec<_>>();

    let diffs = planned
        .iter()
        .map(|change| match change.kind {
            ChangeKind::Add => unified_diff(
                "",
                change.new_content.as_deref().unwrap_or(""),
                &change.display_path,
                /*created*/ true,
            ),
            ChangeKind::Delete => unified_diff(
                change.original_target.as_deref().unwrap_or(""),
                "",
                &change.display_path,
                /*created*/ false,
            ),
            ChangeKind::Update => unified_diff(
                change
                    .original_source
                    .as_deref()
                    .or(change.original_target.as_deref())
                    .unwrap_or(""),
                change.new_content.as_deref().unwrap_or(""),
                &change.display_path,
                /*created*/ false,
            ),
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let display_paths = planned
        .iter()
        .map(|change| change.display_path.clone())
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

async fn plan_hunk(
    hunk: &Hunk,
    resolve_path: &impl Fn(&str) -> Result<PathBuf, ToolError>,
    display_path: &impl Fn(&str) -> String,
) -> Result<PlannedChange, ToolError> {
    match hunk {
        Hunk::Add { path, contents } => {
            let requested = path_string(path)?;
            let target = resolve_path(&requested)?;
            let original_target = match tokio::fs::read_to_string(&target).await {
                Ok(content) => Some(content),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(ToolError::Message(format!(
                        "Failed to read file {}: {error}",
                        display_path(&requested)
                    )))
                }
            };
            Ok(PlannedChange {
                kind: ChangeKind::Add,
                target,
                display_path: display_path(&requested),
                remove_source: None,
                remove_source_display: None,
                original_target,
                original_source: None,
                new_content: Some(contents.clone()),
            })
        }
        Hunk::Delete { path } => {
            let requested = path_string(path)?;
            let target = resolve_path(&requested)?;
            let original_target = tokio::fs::read_to_string(&target).await.map_err(|error| {
                ToolError::Message(format!(
                    "Failed to delete file {}: {error}",
                    display_path(&requested)
                ))
            })?;
            Ok(PlannedChange {
                kind: ChangeKind::Delete,
                target,
                display_path: display_path(&requested),
                remove_source: None,
                remove_source_display: None,
                original_target: Some(original_target),
                original_source: None,
                new_content: None,
            })
        }
        Hunk::Update {
            path,
            move_path,
            chunks,
        } => {
            let requested = path_string(path)?;
            let source = resolve_path(&requested)?;
            let original = tokio::fs::read_to_string(&source).await.map_err(|error| {
                ToolError::Message(format!(
                    "Failed to read file to update {}: {error}",
                    display_path(&requested)
                ))
            })?;
            let new_content = derive_new_contents(&original, &display_path(&requested), chunks)?;
            if let Some(dest) = move_path {
                let dest_requested = path_string(dest)?;
                let target = resolve_path(&dest_requested)?;
                let original_target = match tokio::fs::read_to_string(&target).await {
                    Ok(content) => Some(content),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => {
                        return Err(ToolError::Message(format!(
                            "Failed to read move destination {}: {error}",
                            display_path(&dest_requested)
                        )))
                    }
                };
                Ok(PlannedChange {
                    kind: ChangeKind::Update,
                    target,
                    display_path: display_path(&dest_requested),
                    remove_source: Some(source),
                    remove_source_display: Some(display_path(&requested)),
                    original_target,
                    original_source: Some(original),
                    new_content: Some(new_content),
                })
            } else {
                Ok(PlannedChange {
                    kind: ChangeKind::Update,
                    target: source,
                    display_path: display_path(&requested),
                    remove_source: None,
                    remove_source_display: None,
                    original_target: Some(original),
                    original_source: None,
                    new_content: Some(new_content),
                })
            }
        }
    }
}

fn path_string(path: &Path) -> Result<String, ToolError> {
    let requested = path.to_str().map(str::to_owned).ok_or_else(|| {
        ToolError::Message(format!("path is not valid UTF-8: {}", path.display()))
    })?;
    validate_patch_path(&requested)?;
    Ok(requested)
}

/// Patch paths must stay workspace-relative: no absolute paths and no `..`.
fn validate_patch_path(path: &str) -> Result<(), ToolError> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
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

fn derive_new_contents(
    original_contents: &str,
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<String, ToolError> {
    let mut original_lines: Vec<String> = original_contents.split('\n').map(String::from).collect();
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }
    let replacements = compute_replacements(&original_lines, path, chunks)?;
    let mut new_lines = apply_replacements(original_lines, &replacements);
    if !new_lines.last().is_some_and(String::is_empty) {
        new_lines.push(String::new());
    }
    Ok(new_lines.join("\n"))
}

fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, ToolError> {
    let mut replacements = Vec::new();
    let mut line_index = 0usize;

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
            replacements.push((line_index, 0, chunk.new_lines.clone()));
            line_index = line_index.saturating_add(chunk.new_lines.len());
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
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            line_index = start_idx + pattern.len();
        } else {
            return Err(ToolError::Message(format!(
                "Failed to find expected lines in {path}:\n{}",
                chunk.old_lines.join("\n")
            )));
        }
    }

    replacements.sort_by_key(|(index, _, _)| *index);
    Ok(replacements)
}

fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        let old_len = *old_len;
        for _ in 0..old_len {
            if start_idx < lines.len() {
                lines.remove(start_idx);
            }
        }
        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
        }
    }
    lines
}

async fn commit_changes(planned: &[PlannedChange]) -> Result<(), ToolError> {
    let mut applied = Vec::new();
    for change in planned {
        let result = apply_one(change).await;
        match result {
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

async fn apply_one(change: &PlannedChange) -> Result<(), ToolError> {
    match change.kind {
        ChangeKind::Add | ChangeKind::Update => {
            if let Some(parent) = change.target.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|error| {
                    ToolError::Message(format!(
                        "failed to create parent directories for {}: {error}",
                        change.display_path
                    ))
                })?;
            }
            let content = change.new_content.as_deref().unwrap_or("");
            tokio::fs::write(&change.target, content)
                .await
                .map_err(|error| {
                    ToolError::Message(format!("failed to write {}: {error}", change.display_path))
                })?;
            if let Some(source) = &change.remove_source {
                if source != &change.target {
                    tokio::fs::remove_file(source).await.map_err(|error| {
                        ToolError::Message(format!(
                            "failed to remove moved source {}: {error}",
                            change.source_display()
                        ))
                    })?;
                }
            }
            Ok(())
        }
        ChangeKind::Delete => tokio::fs::remove_file(&change.target)
            .await
            .map_err(|error| {
                ToolError::Message(format!("failed to delete {}: {error}", change.display_path))
            }),
    }
}

async fn rollback_applied(applied: &[&PlannedChange]) -> Result<(), ToolError> {
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

async fn rollback_one(change: &PlannedChange) -> Result<(), ToolError> {
    match change.kind {
        ChangeKind::Add => {
            if change.original_target.is_none() {
                let _ = tokio::fs::remove_file(&change.target).await;
                Ok(())
            } else if let Some(content) = &change.original_target {
                tokio::fs::write(&change.target, content)
                    .await
                    .map_err(|error| {
                        ToolError::Message(format!("{}: {error}", change.display_path))
                    })
            } else {
                Ok(())
            }
        }
        ChangeKind::Delete => {
            if let Some(content) = &change.original_target {
                if let Some(parent) = change.target.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|error| {
                        ToolError::Message(format!("{}: {error}", change.display_path))
                    })?;
                }
                tokio::fs::write(&change.target, content)
                    .await
                    .map_err(|error| {
                        ToolError::Message(format!("{}: {error}", change.display_path))
                    })
            } else {
                Ok(())
            }
        }
        ChangeKind::Update => {
            if let Some(source) = &change.remove_source {
                if let Some(content) = &change.original_source {
                    if let Some(parent) = source.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(|error| {
                            ToolError::Message(format!("{}: {error}", change.source_display()))
                        })?;
                    }
                    tokio::fs::write(source, content).await.map_err(|error| {
                        ToolError::Message(format!("{}: {error}", change.source_display()))
                    })?;
                }
                match &change.original_target {
                    Some(content) => {
                        tokio::fs::write(&change.target, content)
                            .await
                            .map_err(|error| {
                                ToolError::Message(format!("{}: {error}", change.display_path))
                            })
                    }
                    None => {
                        let _ = tokio::fs::remove_file(&change.target).await;
                        Ok(())
                    }
                }
            } else if let Some(content) = &change.original_target {
                tokio::fs::write(&change.target, content)
                    .await
                    .map_err(|error| {
                        ToolError::Message(format!("{}: {error}", change.display_path))
                    })
            } else {
                Ok(())
            }
        }
    }
}
