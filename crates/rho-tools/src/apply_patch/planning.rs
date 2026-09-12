//! Resolve paths through the caller's policy and build a conflict-free change plan.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::tool::ToolError;

use super::{
    content::derive_new_contents,
    model::{FileChange, MoveSource},
    parser::Hunk,
};

pub(super) async fn plan_hunks(
    hunks: &[Hunk],
    resolve_path: &impl Fn(&str) -> Result<PathBuf, ToolError>,
    display_path: &impl Fn(&str) -> String,
) -> Result<Vec<FileChange>, ToolError> {
    let mut planned = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        planned.push(plan_hunk(hunk, resolve_path, display_path).await?);
    }
    check_path_conflicts(&planned)?;
    Ok(planned)
}

fn check_path_conflicts(changes: &[FileChange]) -> Result<(), ToolError> {
    let mut writes = BTreeMap::<PathBuf, String>::new();
    let mut deletes = BTreeMap::<PathBuf, String>::new();

    for change in changes {
        if let Some((path, display)) = change.write_target() {
            if let Some(previous) = writes.insert(path.to_path_buf(), display.to_string()) {
                return Err(ToolError::Message(format!(
                    "patch targets '{display}' more than once (also as '{previous}')"
                )));
            }
        }
        if let Some((path, display)) = change.delete_target() {
            if let Some(previous) = deletes.insert(path.to_path_buf(), display.to_string()) {
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
            let display = display_path(path);
            let target = resolve_path(path)?;
            // Dangling symlink leaves are invisible to read_optional (NotFound).
            if std::fs::symlink_metadata(&target)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                return Err(ToolError::Message(format!(
                    "Refusing to add '{display}': file already exists"
                )));
            }
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
            let display = display_path(path);
            let target = resolve_path(path)?;
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
            let source_display = display_path(path);
            let source = resolve_path(path)?;
            if move_path.is_some() {
                reject_symlink_entry(&source, &source_display)?;
            }
            let permissions = read_permissions(&source, &source_display).await?;
            let old_content = read_required(&source, &source_display, RequiredRead::Update).await?;
            let new_content = derive_new_contents(&old_content, &source_display, chunks)?;
            if let Some(dest) = move_path {
                let target = resolve_path(dest)?;
                let dest_display = display_path(dest);
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
