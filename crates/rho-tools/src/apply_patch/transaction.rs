//! Transaction commit, filesystem mutation, and rollback.
//!
//! Additions and in-place updates whose planned [`TextRoute`] is the host go
//! through it (see [`crate::host_files`]); everything else, including every
//! deletion and move, runs on disk.

use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    file_mutation::{
        atomic_create_file, create_missing_parents, lock_for_rewrite, locked_path_identity_matches,
        rewrite_locked_file_tracked, AtomicCreateEffects, AtomicCreateFailure,
        AtomicCreateFaultInjector, AtomicCreateTargetEffect, RewriteFailure, RewriteFaultInjector,
    },
    host_files::{CommitError, TextRoute},
    tool::ToolError,
};

use super::model::{FileChange, MoveSource};

pub(super) type RewriteFault = Option<Arc<dyn RewriteFaultInjector>>;
pub(super) type CreateFault = Option<Arc<dyn AtomicCreateFaultInjector>>;

#[derive(Clone, Copy)]
enum RollbackTarget<'a> {
    NoChange,
    Change(&'a FileChange),
}

struct TransactionEffects<'a> {
    target: RollbackTarget<'a>,
    created_directories: Vec<PathBuf>,
    residual_files: Vec<PathBuf>,
    /// The text the host holds after writing the change, which it may have
    /// formatted; `None` for changes on disk.
    saved: Option<String>,
}

impl<'a> TransactionEffects<'a> {
    fn for_change(change: &'a FileChange, effects: AtomicCreateEffects) -> Self {
        Self {
            target: RollbackTarget::Change(change),
            created_directories: effects.created_directories,
            residual_files: effects.residual_files,
            saved: None,
        }
    }

    fn change_only(change: &'a FileChange) -> Self {
        Self::for_change(change, AtomicCreateEffects::default())
    }

    fn saved(mut self, saved: String) -> Self {
        self.saved = Some(saved);
        self
    }

    fn creation_failure(change: &'a FileChange, effects: AtomicCreateEffects) -> Self {
        let target = match effects.target {
            AtomicCreateTargetEffect::Unchanged => RollbackTarget::NoChange,
            AtomicCreateTargetEffect::Installed => RollbackTarget::Change(change),
        };
        Self {
            target,
            created_directories: effects.created_directories,
            residual_files: effects.residual_files,
            saved: None,
        }
    }

    fn cleanup_only(effects: AtomicCreateEffects) -> Self {
        Self {
            target: RollbackTarget::NoChange,
            created_directories: effects.created_directories,
            residual_files: effects.residual_files,
            saved: None,
        }
    }
}

/// State left by a failed operation before transaction rollback starts.
enum FailedMutation<'a> {
    /// This operation has no filesystem effect to roll back.
    Clean,
    /// This operation left tracked entries that rollback must remove or restore.
    Effects(TransactionEffects<'a>),
    /// These targets contain partial or otherwise unrestored bytes.
    Dirty(Vec<String>),
}

struct ApplyFailure<'a> {
    error: ToolError,
    mutation: FailedMutation<'a>,
}

impl<'a> ApplyFailure<'a> {
    fn clean(error: ToolError) -> Self {
        Self {
            error,
            mutation: FailedMutation::Clean,
        }
    }

    fn from_create(error: AtomicCreateFailure, change: &'a FileChange) -> Self {
        if error.effects.is_empty() {
            return Self::clean(error.error);
        }
        Self {
            error: error.error,
            mutation: FailedMutation::Effects(TransactionEffects::creation_failure(
                change,
                error.effects,
            )),
        }
    }
    fn change_requires_rollback(
        error: ToolError,
        change: &'a FileChange,
        effects: AtomicCreateEffects,
    ) -> Self {
        Self {
            error,
            mutation: FailedMutation::Effects(TransactionEffects::for_change(change, effects)),
        }
    }

    fn from_rewrite(error: RewriteFailure, display_path: &str) -> Self {
        match error {
            RewriteFailure::Unchanged(error) => Self::clean(error),
            RewriteFailure::Restored(error) => Self::clean(ToolError::Message(format!(
                "{error}; original contents were restored"
            ))),
            RewriteFailure::Dirty {
                error,
                restoration_error,
            } => Self {
                error: ToolError::Message(format!(
                    "{error}; failed to restore original contents: {restoration_error}"
                )),
                mutation: FailedMutation::Dirty(vec![display_path.to_string()]),
            },
        }
    }
}

/// Applies `planned` in order, rolling applied changes back when one fails.
/// Returns, for each change, the text the host holds after writing it, or
/// `None` for a change on disk.
pub(super) async fn commit_changes(
    planned: &[FileChange],
    rewrite_fault: RewriteFault,
    create_fault: CreateFault,
) -> Result<Vec<Option<String>>, ToolError> {
    for change in planned {
        revalidate_change(change).await?;
    }

    let mut applied = Vec::new();
    for change in planned {
        match apply_one(change, &rewrite_fault, &create_fault).await {
            Ok(effects) => applied.push(effects),
            Err(failure) => {
                let mut rollback = applied;
                let mut dirty_paths = Vec::new();
                match failure.mutation {
                    FailedMutation::Clean => {}
                    FailedMutation::Effects(effects) => rollback.push(effects),
                    FailedMutation::Dirty(paths) => dirty_paths.extend(paths),
                }

                let rollback_report = rollback_effects(&rollback).await;
                dirty_paths.extend(rollback_report.unrecovered_paths);
                dirty_paths.sort();
                dirty_paths.dedup();

                let mut message = failure.error.to_string();
                if !rollback_report.errors.is_empty() {
                    message = format!(
                        "{message}; rollback also failed: {}",
                        rollback_report.errors.join("; ")
                    );
                } else if !rollback.is_empty() {
                    let rolled_back = if dirty_paths.is_empty() {
                        "applied changes were rolled back"
                    } else {
                        "other applied changes were rolled back"
                    };
                    message = format!("{message}; {rolled_back}");
                }
                if !dirty_paths.is_empty() {
                    message = format!(
                        "{message}; rollback incomplete; unrecovered paths: {}",
                        dirty_paths.join(", ")
                    );
                }
                return Err(ToolError::Message(message));
            }
        }
    }
    Ok(applied.into_iter().map(|effects| effects.saved).collect())
}

/// Whether the host holds a file at `target` that a new file would replace,
/// such as an unsaved buffer with no file on disk, including one past the
/// input limit.
pub(super) async fn host_holds(route: &TextRoute, target: &Path, display_path: &str) -> bool {
    !matches!(route.read_if_held(target, display_path).await, Ok(None))
}

pub(super) fn add_exists_error(display_path: &str) -> ToolError {
    ToolError::Message(format!(
        "Refusing to add '{display_path}': file already exists"
    ))
}

pub(super) fn move_exists_error(display_path: &str) -> ToolError {
    ToolError::Message(format!(
        "Refusing to move to '{display_path}': destination already exists"
    ))
}

async fn revalidate_change(change: &FileChange) -> Result<(), ToolError> {
    match change {
        FileChange::Add {
            target,
            display_path,
            route,
            ..
        } => {
            expect_live(target, display_path, None).await?;
            if host_holds(route, target, display_path).await {
                return Err(add_exists_error(display_path));
            }
            Ok(())
        }
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
            route,
            ..
        } => {
            if let Some(source) = move_from {
                expect_live(&source.path, &source.display_path, Some(&source.content)).await?;
                expect_live(target, display_path, None).await?;
                if host_holds(&source.destination_route, target, display_path).await {
                    return Err(move_exists_error(display_path));
                }
                Ok(())
            } else {
                match route {
                    TextRoute::Disk => expect_live(target, display_path, Some(old_content)).await,
                    route @ TextRoute::Host(_) => {
                        if route.read(target, display_path).await? == *old_content {
                            Ok(())
                        } else {
                            Err(changed_error(display_path))
                        }
                    }
                }
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
    rewrite_fault: &RewriteFault,
) -> Result<(), RewriteFailure> {
    let path = path.to_path_buf();
    let display_path = display_path.to_string();
    let expected = expected.to_string();
    let updated = updated.to_string();
    let rewrite_fault = rewrite_fault.clone();
    tokio::task::spawn_blocking(move || {
        let mut file = lock_for_rewrite(&path, &display_path, " for apply_patch")
            .map_err(RewriteFailure::Unchanged)?;
        if !locked_path_identity_matches(&file, &path, &display_path)
            .map_err(RewriteFailure::Unchanged)?
        {
            return Err(RewriteFailure::Unchanged(changed_error(&display_path)));
        }
        let mut live = String::new();
        file.read_to_string(&mut live).map_err(|error| {
            RewriteFailure::Unchanged(ToolError::Message(format!(
                "failed to re-read {display_path}: {error}"
            )))
        })?;
        if live != expected {
            return Err(RewriteFailure::Unchanged(changed_error(&display_path)));
        }
        rewrite_locked_file_tracked(
            &mut file,
            &display_path,
            /*original*/ &live,
            /*updated*/ &updated,
            rewrite_fault.as_deref(),
        )
    })
    .await
    .map_err(|error| {
        RewriteFailure::Unchanged(ToolError::Message(format!(
            "apply_patch task failed: {error}"
        )))
    })?
}

async fn remove_current(path: &Path, display_path: &str, expected: &str) -> Result<(), ToolError> {
    let path = path.to_path_buf();
    let display_path = display_path.to_string();
    let expected = expected.to_string();
    tokio::task::spawn_blocking(move || {
        let mut file = lock_for_rewrite(&path, &display_path, " for apply_patch")?;
        if !locked_path_identity_matches(&file, &path, &display_path)? {
            return Err(changed_error(&display_path));
        }
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

async fn apply_one<'a>(
    change: &'a FileChange,
    rewrite_fault: &RewriteFault,
    create_fault: &CreateFault,
) -> Result<TransactionEffects<'a>, ApplyFailure<'a>> {
    match change {
        FileChange::Add {
            target,
            display_path,
            new_content,
            route,
        } => match route {
            TextRoute::Host(_) => {
                create_through_host(change, route, target, display_path, new_content).await
            }
            TextRoute::Disk => atomic_create_file(
                target,
                display_path,
                new_content,
                None,
                create_fault.as_deref(),
            )
            .await
            .map(|success| TransactionEffects::for_change(change, success.effects))
            .map_err(|error| ApplyFailure::from_create(error, change)),
        },
        FileChange::Delete {
            target,
            display_path,
            previous_content,
            ..
        } => remove_current(target, display_path, previous_content)
            .await
            .map(|()| TransactionEffects::change_only(change))
            .map_err(ApplyFailure::clean),
        FileChange::Update {
            target,
            display_path,
            old_content,
            new_content,
            move_from,
            route,
            ..
        } => {
            if let Some(source) = move_from {
                let creation = atomic_create_file(
                    target,
                    display_path,
                    new_content,
                    Some(source.permissions.clone()),
                    create_fault.as_deref(),
                )
                .await
                .map_err(|error| ApplyFailure::from_create(error, change))?;
                if source.path != *target {
                    if let Err(error) =
                        remove_current(&source.path, &source.display_path, &source.content).await
                    {
                        return Err(ApplyFailure::change_requires_rollback(
                            error,
                            change,
                            creation.effects,
                        ));
                    }
                }
                Ok(TransactionEffects::for_change(change, creation.effects))
            } else {
                match route {
                    TextRoute::Disk => rewrite_current(
                        target,
                        display_path,
                        /*expected*/ old_content,
                        /*updated*/ new_content,
                        /*rewrite_fault*/ rewrite_fault,
                    )
                    .await
                    .map(|()| TransactionEffects::change_only(change))
                    .map_err(|error| ApplyFailure::from_rewrite(error, display_path)),
                    route @ TextRoute::Host(_) => route
                        .commit(target, display_path, old_content, new_content)
                        .await
                        .map(|saved| TransactionEffects::change_only(change).saved(saved))
                        .map_err(|error| ApplyFailure::clean(commit_error(error, display_path))),
                }
            }
        }
    }
}

/// Creates a new host-served file through the host, after creating its
/// missing parent directories on disk. A failed write is treated as not
/// having created the file; ACP has no way to delete one.
async fn create_through_host<'a>(
    change: &'a FileChange,
    route: &TextRoute,
    target: &Path,
    display_path: &str,
    content: &str,
) -> Result<TransactionEffects<'a>, ApplyFailure<'a>> {
    let created_directories = create_missing_parents(target, display_path)
        .await
        .map_err(|error| ApplyFailure::from_create(error, change))?;
    match route.write(target, display_path, content).await {
        Ok(saved) => Ok(TransactionEffects::for_change(
            change,
            AtomicCreateEffects {
                target: AtomicCreateTargetEffect::Installed,
                created_directories,
                residual_files: Vec::new(),
            },
        )
        .saved(saved)),
        Err(error) => Err(ApplyFailure {
            error,
            mutation: FailedMutation::Effects(TransactionEffects::cleanup_only(
                AtomicCreateEffects {
                    created_directories,
                    ..AtomicCreateEffects::default()
                },
            )),
        }),
    }
}

fn commit_error(error: CommitError, display_path: &str) -> ToolError {
    match error {
        CommitError::PathChanged | CommitError::Changed { .. } => changed_error(display_path),
        CommitError::Failed(error) => error,
    }
}

#[derive(Default)]
struct RollbackReport {
    errors: Vec<String>,
    unrecovered_paths: Vec<String>,
}

async fn rollback_effects(effects: &[TransactionEffects<'_>]) -> RollbackReport {
    let mut report = RollbackReport::default();
    for effect in effects.iter().rev() {
        rollback_effect(effect, &mut report).await;
    }
    report
}

async fn rollback_effect(effect: &TransactionEffects<'_>, report: &mut RollbackReport) {
    let mut left_in_place = false;
    if let RollbackTarget::Change(change) = effect.target {
        if let Err(error) = rollback_one(change, effect.saved.as_deref()).await {
            report.errors.push(error.to_string());
            report
                .unrecovered_paths
                .extend(change.affected_display_paths().map(str::to_string));
            left_in_place = true;
        }
    }
    remove_residual_files(effect, report).await;
    // A file rollback left in place, such as one the host added, keeps the
    // directories created for it.
    if !left_in_place {
        remove_created_directories(effect, report).await;
    }
}

async fn cleanup_created_entries(effect: &TransactionEffects<'_>, report: &mut RollbackReport) {
    remove_residual_files(effect, report).await;
    remove_created_directories(effect, report).await;
}

async fn remove_residual_files(effect: &TransactionEffects<'_>, report: &mut RollbackReport) {
    for path in effect.residual_files.iter().rev() {
        if let Err(error) = tokio::fs::remove_file(path).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                report.errors.push(format!(
                    "failed to remove staged file {}: {error}",
                    path.display()
                ));
                report.unrecovered_paths.push(path.display().to_string());
            }
        }
    }
}

async fn remove_created_directories(effect: &TransactionEffects<'_>, report: &mut RollbackReport) {
    for path in effect.created_directories.iter().rev() {
        if let Err(error) = tokio::fs::remove_dir(path).await {
            if error.kind() == std::io::ErrorKind::NotFound {
                continue;
            }
            let message = if error.kind() == std::io::ErrorKind::DirectoryNotEmpty {
                format!(
                    "created directory {} is no longer empty; refusing to remove concurrent contents",
                    path.display()
                )
            } else {
                format!(
                    "failed to remove created directory {}: {error}",
                    path.display()
                )
            };
            report.errors.push(message);
            report.unrecovered_paths.push(path.display().to_string());
        }
    }
}

/// Undoes one applied change unless another writer changed it since.
/// `saved` is the text the host holds after writing the change, when it went
/// through the host; it is what the file must still hold. A file the host
/// created cannot be removed through it, so it stays and is reported.
pub(super) async fn rollback_one(
    change: &FileChange,
    saved: Option<&str>,
) -> Result<(), ToolError> {
    match change {
        FileChange::Add {
            display_path,
            route,
            ..
        } if route.is_host() => {
            // ACP cannot remove a file, and a host such as Zed keeps serving
            // a buffer whose file was deleted under it. Removing only the
            // disk copy would leave a file the host holds but `apply_patch`
            // can neither add nor update, so the added file stays, reported.
            Err(ToolError::Message(format!(
                "{display_path}: the host holds the added file, and it cannot be removed through the host; it was left in place"
            )))
        }
        FileChange::Add {
            target,
            display_path,
            new_content,
            ..
        } => match read_live(target, display_path).await? {
            LiveText::Missing => Ok(()),
            LiveText::Content(live) if live == saved.unwrap_or(new_content) => {
                remove_current(target, display_path, &live).await
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
                restore_file(
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
            move_from,
            route,
        } => {
            if let Some(source) = move_from {
                rollback_move(source, target, display_path, new_content).await
            } else if let route @ TextRoute::Host(_) = route {
                match route
                    .commit(
                        target,
                        display_path,
                        saved.unwrap_or(new_content),
                        old_content,
                    )
                    .await
                {
                    Ok(_) => Ok(()),
                    Err(CommitError::Changed { live }) if live == *old_content => Ok(()),
                    Err(CommitError::Failed(error)) => Err(error),
                    Err(CommitError::PathChanged | CommitError::Changed { .. }) => {
                        Err(concurrent_rollback_error(display_path))
                    }
                }
            } else {
                match read_live(target, display_path).await? {
                    LiveText::Content(live) if live == *new_content => {
                        rewrite_current(
                            target,
                            display_path,
                            /*expected*/ new_content,
                            /*updated*/ old_content,
                            /*rewrite_fault*/ &None,
                        )
                        .await
                        .map_err(RewriteFailure::into_tool_error)
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
        restore_file(
            &source.path,
            &source.display_path,
            &source.content,
            Some(source.permissions.clone()),
        )
        .await?;
    }
    if matches!(target_live, LiveText::Content(_)) {
        remove_current(target, target_display, new_content).await?;
    }
    Ok(())
}

async fn restore_file(
    path: &Path,
    display_path: &str,
    content: &str,
    permissions: Option<std::fs::Permissions>,
) -> Result<(), ToolError> {
    match atomic_create_file(path, display_path, content, permissions, None).await {
        Ok(_) => Ok(()),
        Err(failure) => {
            let mut report = RollbackReport::default();
            cleanup_created_entries(
                &TransactionEffects::cleanup_only(failure.effects),
                &mut report,
            )
            .await;
            if report.errors.is_empty() {
                Err(failure.error)
            } else {
                Err(ToolError::Message(format!(
                    "{}; cleanup after failed restoration also failed: {}",
                    failure.error,
                    report.errors.join("; ")
                )))
            }
        }
    }
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
