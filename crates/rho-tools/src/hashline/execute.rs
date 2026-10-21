//! Multi-file plan / commit / rollback for prepared hashline sections.
//!
//! Path policy stays with the caller ([`PreparedSection`] is already resolved).
//! Path uniqueness for a mutation is owned here via [`claim_unique_path`] so
//! prepare and execute share one predicate.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use crate::{
    diff::unified_diff,
    file_mutation::FileMutationOutcome,
    host_files::{CommitError, HostTextFiles, TextRoute},
    tool::*,
};

use super::{
    apply::apply_ops,
    format::{format_chain_snapshot, format_post_edit_preview},
    parser::{Op, Section},
};

/// Replace/delete span size that marks a structural edit in the post-edit footer.
const STRUCTURAL_EDIT_SPAN_LINES: usize = 40;

/// One parsed hashline section whose target the caller has already resolved and
/// authorized.
///
/// Callers resolve paths themselves because the workspace-aware adapter must
/// authorize each target before any content is read. Handing the resolved pair
/// in keeps this module free of path policy and avoids re-parsing the document.
pub(crate) struct PreparedSection {
    pub(crate) section: Section,
    pub(crate) path: PathBuf,
    pub(crate) display_path: String,
}

/// Claim `canonical` under document claim `claimed_as`.
///
/// Used by SDK prepare (fail-fast before auth) and by execute (every write path,
/// including the App harness that skips prepare). One predicate, two call sites.
pub(crate) fn claim_unique_path(
    seen: &mut BTreeMap<PathBuf, String>,
    canonical: PathBuf,
    claimed_as: &str,
) -> Result<(), String> {
    use std::collections::btree_map::Entry;
    match seen.entry(canonical) {
        Entry::Vacant(slot) => {
            slot.insert(claimed_as.to_string());
            Ok(())
        }
        Entry::Occupied(slot) => {
            let prior = slot.get();
            if prior == claimed_as {
                Err(format!(
                    "hashline document claims path '{claimed_as}' more than once"
                ))
            } else {
                Err(format!(
                    "hashline document claims path '{claimed_as}' more than once (also as '{prior}')"
                ))
            }
        }
    }
}

/// Apply already-resolved hashline sections to the workspace. Each path goes
/// to the host when `host_files` serves it, and to local disk otherwise.
pub(crate) async fn apply_prepared_sections(
    sections: Vec<PreparedSection>,
    max_output_bytes: usize,
    host_files: Option<&Arc<dyn HostTextFiles>>,
) -> Result<FileMutationOutcome, ToolError> {
    let mut seen = BTreeMap::new();
    for prepared in &sections {
        claim_unique_path(&mut seen, prepared.path.clone(), &prepared.section.path)
            .map_err(ToolError::Message)?;
    }

    // Plan every file first so a later section failure cannot leave earlier
    // writes applied. Each commit re-checks its file, so no separate
    // revalidation pass is needed here.
    let mut planned = Vec::with_capacity(sections.len());
    for prepared in sections {
        let display_path = prepared.display_path.clone();
        let route = TextRoute::for_path(host_files, &prepared.path);
        let original = route.read(&prepared.path, &display_path).await?;
        let anchor_lines = collect_focus_anchors(&prepared.section.ops);
        let structural = ops_are_structural(&prepared.section.ops);
        let ops_summary = format_ops_summary(&prepared.section.ops);
        let outcome = apply_ops(&original, &prepared.section.tag, prepared.section.ops)
            .map_err(|error| recovery_error(&display_path, &original, &anchor_lines, error))?;
        planned.push(PlannedFile {
            route,
            path: prepared.path,
            display_path,
            original,
            outcome,
            anchor_lines,
            ops_summary,
            structural,
        });
    }

    // The commit runs to its end, rollback included, even when the call is
    // cancelled, so a dropped call never leaves only some files committed.
    let planned = tokio::spawn(commit_planned(planned))
        .await
        .map_err(|error| ToolError::Message(format!("edit task failed: {error}")))??;

    let mut previews = Vec::new();
    let mut diffs = Vec::new();
    for file in &planned {
        let preview = format_post_edit_preview(
            &file.display_path,
            &file.outcome.text,
            &file.outcome.focus_lines,
            file.structural,
        );
        previews.push(format!("{}\n{preview}", file.ops_summary));
        diffs.push(unified_diff(
            &file.original,
            &file.outcome.text,
            &file.display_path,
            /*created*/ false,
        ));
    }

    let diff = diffs.join("\n");
    let content = truncate(previews.join("\n\n"), max_output_bytes);
    Ok(FileMutationOutcome {
        content,
        display_paths: planned
            .iter()
            .map(|file| file.display_path.clone())
            .collect(),
        diff,
        file_changes: planned
            .into_iter()
            .map(|file| {
                rho_sdk::tool::ToolFileChange::modified(file.path, file.original, file.outcome.text)
            })
            .collect(),
    })
}

/// Commits every planned file in order, rolling back the committed ones
/// when one fails. Returns the files holding the text each now has.
async fn commit_planned(mut planned: Vec<PlannedFile>) -> Result<Vec<PlannedFile>, ToolError> {
    for index in 0..planned.len() {
        match commit_planned_file(&planned[index]).await {
            // What the file holds now, which a host may have formatted, is
            // what rollback must find and what the result shows.
            Ok(written) => planned[index].record_written(written),
            Err(error) => {
                let applied = &planned[..index];
                let mut message = error.to_string();
                if let Err(rollback_error) = rollback_applied(applied).await {
                    message = format!("{message}; rollback also failed: {rollback_error}");
                } else if !applied.is_empty() {
                    message = format!("{message}; applied changes were rolled back");
                }
                return Err(ToolError::Message(message));
            }
        }
    }
    Ok(planned)
}

/// One file's planned rewrite, before anything is written.
struct PlannedFile {
    route: TextRoute,
    path: PathBuf,
    display_path: String,
    original: String,
    outcome: super::apply::ApplyOutcome,
    anchor_lines: Vec<usize>,
    ops_summary: String,
    structural: bool,
}

impl PlannedFile {
    /// Records the text the file holds after its commit. A host that
    /// reformatted it may have moved the edited lines, so the preview then
    /// focuses on the span the saved text changed.
    fn record_written(&mut self, written: String) {
        if written != self.outcome.text {
            self.outcome.focus_lines = changed_span(&self.original, &written);
            self.outcome.text = written;
        }
    }
}

/// The first and last 1-indexed lines of `updated` that differ from
/// `original`, or none when the texts match.
pub(super) fn changed_span(original: &str, updated: &str) -> Vec<usize> {
    let (before, after) = (
        original.lines().collect::<Vec<_>>(),
        updated.lines().collect::<Vec<_>>(),
    );
    let prefix = before
        .iter()
        .zip(&after)
        .take_while(|(old, new)| old == new)
        .count();
    let suffix = before[prefix..]
        .iter()
        .rev()
        .zip(after[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let last = after.len() - suffix;
    if last > prefix {
        vec![prefix + 1, last]
    } else if original == updated {
        Vec::new()
    } else {
        // Lines were only removed; focus where they were.
        vec![(prefix + 1).min(after.len().max(1))]
    }
}

async fn commit_planned_file(file: &PlannedFile) -> Result<String, ToolError> {
    let display_path = &file.display_path;
    file.route
        .commit(&file.path, display_path, &file.original, &file.outcome.text)
        .await
        .map_err(|error| match error {
            CommitError::PathChanged => recovery_error(
                display_path,
                &file.original,
                &file.anchor_lines,
                "path changed after validation",
            ),
            CommitError::Changed { live } => recovery_error(
                display_path,
                &live,
                &file.anchor_lines,
                "changed during edit",
            ),
            CommitError::Failed(error) => error,
        })
}

fn recovery_error(
    display_path: &str,
    live_text: &str,
    focus_lines: &[usize],
    message: impl std::fmt::Display,
) -> ToolError {
    let snapshot = format_chain_snapshot(display_path, live_text, focus_lines);
    ToolError::Message(format!(
        "{display_path}: {message}\n\nLive snapshot - copy the header and line numbers below to retry:\n{snapshot}"
    ))
}

async fn rollback_applied(applied: &[PlannedFile]) -> Result<(), ToolError> {
    let mut failures = Vec::new();
    for file in applied.iter().rev() {
        if let Err(error) = rollback_one(file).await {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ToolError::Message(failures.join("; ")))
    }
}

/// Restores a committed file, unless another writer changed it after the
/// commit; restoring then would clobber that writer.
async fn rollback_one(file: &PlannedFile) -> Result<(), ToolError> {
    let display_path = &file.display_path;
    file.route
        .commit(&file.path, display_path, &file.outcome.text, &file.original)
        .await
        .map(drop)
        .map_err(|error| match error {
            CommitError::PathChanged => ToolError::Message(format!(
                "{display_path}: path changed after apply; refusing rollback"
            )),
            CommitError::Changed { .. } => ToolError::Message(format!(
                "{display_path}: changed after apply; refusing rollback to avoid clobbering concurrent writers"
            )),
            CommitError::Failed(error) => ToolError::Message(format!(
                "could not roll back {display_path}: {error}"
            )),
        })
}

/// Span endpoints and insert anchors for focused live snapshots on failure.
fn collect_focus_anchors(ops: &[Op]) -> Vec<usize> {
    let mut lines = Vec::new();
    for op in ops {
        match op {
            Op::Replace { start, end, .. } | Op::Delete { start, end } => {
                lines.push(*start);
                if *end != *start {
                    lines.push(*end);
                }
            }
            Op::InsertBefore { line, .. }
            | Op::InsertAfter {
                line: Some(line), ..
            } => lines.push(*line),
            Op::InsertAfter { line: None, .. } => {}
        }
    }
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// True when any single destructive span covers enough original lines that a
/// follow-up edit should re-read rather than trust a focused preview alone.
fn ops_are_structural(ops: &[Op]) -> bool {
    ops.iter().any(|op| match op {
        Op::Replace { start, end, .. } | Op::Delete { start, end } => {
            end.saturating_sub(*start).saturating_add(1) >= STRUCTURAL_EDIT_SPAN_LINES
        }
        Op::InsertBefore { .. } | Op::InsertAfter { .. } => false,
    })
}

/// One-line summary of applied ops using wire locator forms.
fn format_ops_summary(ops: &[Op]) -> String {
    use super::format::{format_cut_locator, format_put_locator};
    let mut parts = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            Op::Replace { start, end, body } => {
                let removed = end.saturating_sub(*start).saturating_add(1);
                let locator = format_put_locator(*start, *end);
                parts.push(format!("{locator}: ({removed} → {} line(s))", body.len()));
            }
            Op::Delete { start, end } => {
                let removed = end.saturating_sub(*start).saturating_add(1);
                let locator = format_cut_locator(*start, *end);
                let unit = if removed == 1 { "line" } else { "lines" };
                parts.push(format!("{locator} ({removed} {unit})"));
            }
            Op::InsertBefore { line, body } => {
                parts.push(format!("PUT <{line}: (+{} line(s))", body.len()));
            }
            Op::InsertAfter {
                line: Some(line),
                body,
            } => {
                parts.push(format!("PUT >{line}: (+{} line(s))", body.len()));
            }
            Op::InsertAfter { line: None, body } => {
                parts.push(format!("PUT >$: (+{} line(s))", body.len()));
            }
        }
    }
    parts.join("; ")
}
