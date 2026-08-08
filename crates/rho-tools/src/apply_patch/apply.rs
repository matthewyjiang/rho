//! Apply parsed Codex-style patch hunks to the filesystem.
//!
//! Pipeline:
//! 1. plan all changes and reject conflicts
//! 2. derive presentation output from the plan
//! 3. revalidate and commit in patch order
//! 4. roll back complete mutations and report any dirty targets

use std::path::PathBuf;

#[cfg(test)]
use std::sync::Arc;

use crate::{
    file_mutation::FileMutationOutcome,
    tool::{truncate, ToolError},
};

#[cfg(test)]
use crate::file_mutation::{AtomicCreateFaultInjector, RewriteFaultInjector};

use super::{
    parser::Hunk,
    planning::plan_hunks,
    transaction::{commit_changes, CreateFault, RewriteFault},
};

pub(super) use super::model::FileChange;
pub(crate) use super::planning::{reject_symlink_entry, validate_hunk_paths};
#[cfg(test)]
pub(super) use super::transaction::rollback_one;

pub(crate) async fn apply_hunks(
    hunks: Vec<Hunk>,
    resolve_path: impl Fn(&str) -> Result<PathBuf, ToolError>,
    display_path: impl Fn(&str) -> String,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError> {
    apply_hunks_inner(
        hunks,
        resolve_path,
        display_path,
        max_output_bytes,
        /*rewrite_fault*/ None,
        /*create_fault*/ None,
    )
    .await
}

#[cfg(test)]
pub(super) async fn apply_hunks_with_faults(
    hunks: Vec<Hunk>,
    resolve_path: impl Fn(&str) -> Result<PathBuf, ToolError>,
    display_path: impl Fn(&str) -> String,
    max_output_bytes: usize,
    rewrite_fault: Option<Arc<dyn RewriteFaultInjector>>,
    create_fault: Option<Arc<dyn AtomicCreateFaultInjector>>,
) -> Result<FileMutationOutcome, ToolError> {
    apply_hunks_inner(
        hunks,
        resolve_path,
        display_path,
        max_output_bytes,
        rewrite_fault,
        create_fault,
    )
    .await
}

async fn apply_hunks_inner(
    hunks: Vec<Hunk>,
    resolve_path: impl Fn(&str) -> Result<PathBuf, ToolError>,
    display_path: impl Fn(&str) -> String,
    max_output_bytes: usize,
    rewrite_fault: RewriteFault,
    create_fault: CreateFault,
) -> Result<FileMutationOutcome, ToolError> {
    let planned = plan_hunks(&hunks, &resolve_path, &display_path).await?;

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

    commit_changes(&planned, rewrite_fault, create_fault).await?;

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
