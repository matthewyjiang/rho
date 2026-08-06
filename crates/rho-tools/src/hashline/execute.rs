//! Multi-file plan / lock / commit / rollback for prepared hashline sections.
//!
//! Path policy stays with the caller ([`PreparedSection`] is already resolved).
//! Path uniqueness for a mutation is owned here via [`claim_unique_path`] so
//! prepare and execute share one predicate.

use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use crate::{diff::unified_diff, file_mutation::FileMutationOutcome, tool::*};

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

/// Apply already-resolved hashline sections to the workspace.
pub(crate) async fn apply_prepared_sections(
    sections: Vec<PreparedSection>,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError> {
    let mut seen = BTreeMap::new();
    for prepared in &sections {
        claim_unique_path(&mut seen, prepared.path.clone(), &prepared.section.path)
            .map_err(ToolError::Message)?;
    }

    tokio::task::spawn_blocking(move || apply_sections_locked(sections, max_output_bytes))
        .await
        .map_err(|error| ToolError::Message(format!("hashline edit task failed: {error}")))?
}

/// One file's planned rewrite, before anything is written.
struct PlannedFile {
    path: PathBuf,
    display_path: String,
    original: String,
    outcome: super::apply::ApplyOutcome,
    anchor_lines: Vec<usize>,
    ops_summary: String,
    structural: bool,
}

/// A file already rewritten in this call, kept so a later failure can undo it.
struct AppliedFile<'a> {
    path: &'a Path,
    display_path: &'a str,
    original: &'a str,
    /// Committed text; rollback refuses to restore if the live file no longer
    /// matches this (another writer raced after our commit).
    applied_text: &'a str,
}

fn apply_sections_locked(
    sections: Vec<PreparedSection>,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError> {
    // Plan every file first so a later section failure cannot leave earlier
    // writes applied. Each commit re-checks its file under lock, so no separate
    // revalidation pass is needed here.
    let mut planned = Vec::with_capacity(sections.len());
    for prepared in sections {
        let display_path = prepared.display_path.clone();
        let original = std::fs::read_to_string(&prepared.path).map_err(|error| {
            ToolError::Message(format!("could not read {display_path}: {error}"))
        })?;
        let anchor_lines = collect_focus_anchors(&prepared.section.ops);
        let structural = ops_are_structural(&prepared.section.ops);
        let ops_summary = format_ops_summary(&prepared.section.ops);
        let outcome = apply_ops(&original, &prepared.section.tag, prepared.section.ops)
            .map_err(|error| recovery_error(&display_path, &original, &anchor_lines, error))?;
        planned.push(PlannedFile {
            path: prepared.path,
            display_path,
            original,
            outcome,
            anchor_lines,
            ops_summary,
            structural,
        });
    }

    let mut applied: Vec<AppliedFile<'_>> = Vec::new();
    for file in &planned {
        if let Err(error) = commit_planned_file(file) {
            let mut message = error.to_string();
            if let Err(rollback_error) = rollback_applied(&applied) {
                message = format!("{message}; rollback also failed: {rollback_error}");
            } else if !applied.is_empty() {
                message = format!("{message}; applied changes were rolled back");
            }
            return Err(ToolError::Message(message));
        }
        applied.push(AppliedFile {
            path: &file.path,
            display_path: &file.display_path,
            original: &file.original,
            applied_text: &file.outcome.text,
        });
    }

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
    })
}

fn commit_planned_file(file: &PlannedFile) -> Result<(), ToolError> {
    let display_path = &file.display_path;
    let mut handle = lock_for_rewrite(&file.path, display_path, "")?;
    let mut live = String::new();
    handle
        .read_to_string(&mut live)
        .map_err(|error| ToolError::Message(format!("could not read {display_path}: {error}")))?;
    if live != file.original {
        return Err(recovery_error(
            display_path,
            &live,
            &file.anchor_lines,
            "changed during edit",
        ));
    }
    rewrite_locked_file(&mut handle, display_path, &file.outcome.text)
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

fn rollback_applied(applied: &[AppliedFile<'_>]) -> Result<(), ToolError> {
    let mut failures = Vec::new();
    for file in applied.iter().rev() {
        if let Err(error) = rollback_one(file) {
            failures.push(error.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ToolError::Message(failures.join("; ")))
    }
}

fn rollback_one(file: &AppliedFile<'_>) -> Result<(), ToolError> {
    let mut handle = lock_for_rewrite(file.path, file.display_path, " for rollback")?;
    let mut live = String::new();
    handle.read_to_string(&mut live).map_err(|error| {
        ToolError::Message(format!(
            "could not read {} for rollback: {error}",
            file.display_path
        ))
    })?;
    if live != file.applied_text {
        return Err(ToolError::Message(format!(
            "{}: changed after apply; refusing rollback to avoid clobbering concurrent writers",
            file.display_path
        )));
    }
    rewrite_locked_file(&mut handle, file.display_path, file.original)
}

fn lock_for_rewrite(
    path: &Path,
    display_path: &str,
    context: &str,
) -> Result<std::fs::File, ToolError> {
    use std::time::{Duration, Instant};

    const LOCK_TIMEOUT: Duration = Duration::from_secs(1);
    const INITIAL_BACKOFF: Duration = Duration::from_millis(2);
    const MAX_BACKOFF: Duration = Duration::from_millis(50);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            ToolError::Message(format!("could not open {display_path}{context}: {error}"))
        })?;

    let deadline = Instant::now() + LOCK_TIMEOUT;
    let mut backoff = INITIAL_BACKOFF;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(ToolError::Message(format!(
                        "could not lock {display_path}{context}: timed out after {}ms",
                        LOCK_TIMEOUT.as_millis()
                    )));
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(ToolError::Message(format!(
                    "could not lock {display_path}{context}: {error}"
                )));
            }
        }
    }
}

fn rewrite_locked_file(
    file: &mut std::fs::File,
    display_path: &str,
    contents: &str,
) -> Result<(), ToolError> {
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        ToolError::Message(format!("could not rewrite {display_path}: {error}"))
    })?;
    file.set_len(0).map_err(|error| {
        ToolError::Message(format!("could not rewrite {display_path}: {error}"))
    })?;
    file.write_all(contents.as_bytes())
        .map_err(|error| ToolError::Message(format!("could not write {display_path}: {error}")))?;
    file.flush()
        .map_err(|error| ToolError::Message(format!("could not write {display_path}: {error}")))?;
    Ok(())
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
