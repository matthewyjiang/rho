//! Line-anchored multi-hunk edit tool (`edit`) with snapshot tags.
//!
//! `read_file` / `grep` / `write_file` mint `[path#TAG]` snapshots. `edit`
//! applies a compact PUT/CUT document against those original line numbers,
//! rejects stale tags (or remaps them via the session snapshot store), and
//! refuses anchors the model never saw under a tagged read.

mod apply;
mod format;
mod parser;
mod proposed;
mod recovery;
mod store;

use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    sync::Arc,
};

use serde::Deserialize;
use serde_json::json;

use crate::{diff::unified_diff, tool::*, write_file::FileMutationOutcome};

use apply::{apply_ops, ApplyOutcome};
use format::format_post_edit_preview;
use parser::{Op, Section};
use recovery::{collect_anchor_lines, try_recover, RECOVERY_REMAP_NOTICE};
pub use store::{HashlineMetrics, HashlineMetricsSnapshot, Snapshot, SnapshotStore};

pub(crate) use format::{compute_file_hash, format_chain_snapshot, format_hashline_view};

/// Line count helper for store recording outside the hashline module.
pub(crate) fn split_content_lines_for_store(text: &str) -> usize {
    format::split_content_lines(text).len()
}
pub(crate) use parser::parse_hashline;
pub use proposed::{
    proposed_edit, proposed_sections, ProposedEdit, ProposedEditFile, ProposedSection,
};

pub(crate) struct Edit;

const TOOL_DESCRIPTION: &str = r#"Use `edit` for multi-hunk edits to existing UTF-8 files when you already have a fresh `[path#TAG]` from `read_file`, `grep` (content mode), a successful `edit` preview, a `write_file` chain snapshot, or a failed `edit` recovery snapshot. Never invent a TAG. Prefer `write_file` to create or fully rewrite a file.

Document shape:

[path#TAG]
PUT N.=M:
+new line
+another line
PUT >N:
+inserted after N
PUT <N:
+inserted before N
PUT >$:
+appended at end
CUT N.=M

Rules:
- Copy the exact `[path#TAG]` header and `N:line` numbers from the latest snapshot for that path
- Put every hunk for one file in a single `edit` document. Do not issue two `edit` tool calls on the same path in one batch - wait for the result before editing that path again. Different paths may edit in parallel
- Line numbers name ORIGINAL lines from that snapshot; they do not shift mid-document
- Body rows under PUT headers that end with `:` must start with `+` (use `+` alone for a blank line)
- Ranges are inclusive (`PUT 3.=5:` touches original lines 3 through 5). `N-M` and `N..M` are also accepted
- Single-line replace may use `PUT N:` as shorthand for `PUT N.=N:`
- PUT always needs at least one + body row; use CUT to delete
- Stale TAG, out-of-range, overlap, and mid-edit file changes fail closed with no write and include a bounded live snapshot - copy that header/lines to retry (re-read only for lines outside the snapshot). When a session snapshot still matches the tag, `edit` may remap anchors onto the live file automatically
- Only edit lines you have seen under that TAG (from read/grep/write/edit output). Unseen anchors are rejected with a short reveal
- An insert anchored inside a range another op replaces or deletes is rejected; anchor it outside
- TAG ignores trailing whitespace, so whitespace-only changes keep a read valid
- Successful results return a post-edit `[path#NEW_TAG]` numbered preview around the change
- Block ops (`N*`), registers (`@name`), REM, and MV are not supported yet
- Create files with `write_file`; do not use `edit` to create paths"#;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    input: String,
}

#[async_trait::async_trait]
impl Tool for Edit {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".into(),
            description: TOOL_DESCRIPTION.into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Hashline document with one or more [path#TAG] sections and PUT/CUT ops. Copy each [path#TAG] from read_file, grep, write_file, a prior edit preview, or a failed edit recovery snapshot; never invent tags. One section per path; multi-hunk OK. Do not stack two edit calls on the same path in one batch."
                    }
                },
                "required": ["input"],
                "additionalProperties": false
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let sections = parse_hashline(&args.input)
            .map_err(|error| ToolError::Message(error.to_string()))?
            .into_iter()
            .map(|section| PreparedSection {
                path: resolve_path(&ctx.cwd, &section.path),
                display_path: compact_display_path(&ctx.cwd, &section.path),
                section,
            })
            .collect();
        let outcome = apply_prepared_sections(sections, ctx.max_output_bytes, None).await?;
        Ok(ToolResult {
            id,
            ok: true,
            content: outcome.content,
        })
    }
}

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

/// Apply already-resolved hashline sections to the workspace.
pub(crate) async fn apply_prepared_sections(
    sections: Vec<PreparedSection>,
    max_output_bytes: usize,
    store: Option<Arc<SnapshotStore>>,
) -> Result<FileMutationOutcome, ToolError> {
    let mut seen = BTreeMap::<&Path, &str>::new();
    for prepared in &sections {
        if let Some(prior) = seen.insert(&prepared.path, &prepared.section.path) {
            return Err(ToolError::Message(format!(
                "hashline document claims path '{}' more than once (also as '{prior}')",
                prepared.section.path
            )));
        }
    }

    tokio::task::spawn_blocking(move || apply_sections_locked(sections, max_output_bytes, store))
        .await
        .map_err(|error| ToolError::Message(format!("hashline edit task failed: {error}")))?
}

/// One file's planned rewrite, before anything is written.
struct PlannedFile {
    path: PathBuf,
    display_path: String,
    original: String,
    outcome: ApplyOutcome,
    anchor_lines: Vec<usize>,
    recovered: bool,
}

/// A file already rewritten in this call, kept so a later failure can undo it.
struct AppliedFile<'a> {
    path: &'a Path,
    display_path: &'a str,
    original: &'a str,
}

fn apply_sections_locked(
    sections: Vec<PreparedSection>,
    max_output_bytes: usize,
    store: Option<Arc<SnapshotStore>>,
) -> Result<FileMutationOutcome, ToolError> {
    // Plan every file first so a later section failure cannot leave earlier
    // writes applied. Each commit re-checks its file under lock, so no separate
    // revalidation pass is needed here.
    let mut planned = Vec::with_capacity(sections.len());
    for prepared in &sections {
        let display_path = &prepared.display_path;
        let original = std::fs::read_to_string(&prepared.path).map_err(|error| {
            ToolError::Message(format!("could not read {display_path}: {error}"))
        })?;
        let focus_anchors = focus_anchor_lines(&prepared.section.ops);
        let (outcome, recovered) =
            plan_one_file(prepared, &original, &focus_anchors, store.as_deref())?;
        planned.push(PlannedFile {
            path: prepared.path.clone(),
            display_path: display_path.clone(),
            original,
            outcome,
            anchor_lines: focus_anchors,
            recovered,
        });
    }

    let mut applied: Vec<AppliedFile<'_>> = Vec::new();
    let mut previews = Vec::new();
    let mut diffs = Vec::new();
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
        });
        if let Some(store) = store.as_ref() {
            let line_count = format::split_content_lines(&file.outcome.text).len();
            let seen = (1..=line_count).collect::<Vec<_>>();
            store.record(&file.path, file.outcome.text.clone(), Some(seen));
        }
        let mut preview = format_post_edit_preview(
            &file.display_path,
            &file.outcome.text,
            &file.outcome.focus_lines,
        );
        if file.recovered {
            preview = format!("{RECOVERY_REMAP_NOTICE}\n\n{preview}");
        }
        previews.push(preview);
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

fn plan_one_file(
    prepared: &PreparedSection,
    original: &str,
    focus_anchors: &[usize],
    store: Option<&SnapshotStore>,
) -> Result<(ApplyOutcome, bool), ToolError> {
    let display_path = &prepared.display_path;
    match apply_ops(original, &prepared.section.tag, &prepared.section.ops) {
        Ok(outcome) => {
            if let Some(store) = store {
                reject_unseen_anchors(store, &prepared.path, original, &prepared.section.ops)?;
            }
            Ok((outcome, false))
        }
        Err(error) if error.contains("tag mismatch") => {
            if let Some(store) = store {
                store.metrics().tag_mismatch.fetch_add(1, Ordering::Relaxed);
                if let Some(snapshot) = store.by_hash(&prepared.path, &prepared.section.tag) {
                    if let Some(outcome) =
                        try_recover(&snapshot.text, original, &prepared.section.ops)
                    {
                        store.metrics().recovery_ok.fetch_add(1, Ordering::Relaxed);
                        return Ok((outcome, true));
                    }
                    store
                        .metrics()
                        .recovery_fail
                        .fetch_add(1, Ordering::Relaxed);
                } else {
                    store
                        .metrics()
                        .recovery_fail
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(recovery_error(display_path, original, focus_anchors, error))
        }
        Err(error) => Err(recovery_error(display_path, original, focus_anchors, error)),
    }
}

/// Cap on unseen anchor lines revealed inline in the rejection.
const UNSEEN_REVEAL_CAP: usize = 12;

fn reject_unseen_anchors(
    store: &SnapshotStore,
    path: &Path,
    original: &str,
    ops: &[Op],
) -> Result<(), ToolError> {
    let Some(snapshot) = store.by_content(path, original) else {
        return Ok(());
    };
    let Some(seen) = snapshot.seen_lines.as_ref() else {
        return Ok(());
    };
    let anchors = collect_anchor_lines(ops);
    let unseen: Vec<usize> = anchors
        .into_iter()
        .filter(|line| !seen.contains(line))
        .collect();
    if unseen.is_empty() {
        return Ok(());
    }
    store
        .metrics()
        .unseen_reject
        .fetch_add(1, Ordering::Relaxed);
    let lines = format::split_content_lines(original);
    let mut reveal = String::new();
    let show = unseen.len().min(UNSEEN_REVEAL_CAP);
    for &line in &unseen[..show] {
        let body = lines.get(line - 1).copied().unwrap_or("");
        reveal.push_str(&format!("\n{line}:{body}"));
    }
    let more = if unseen.len() > show {
        format!("\n… and {} more unseen anchors", unseen.len() - show)
    } else {
        String::new()
    };
    // Merge revealed lines so a retry after this error can proceed.
    store.record_seen_lines(path, &snapshot.hash, unseen.iter().copied().take(show));
    Err(ToolError::Message(format!(
        "{}: edit anchors lines the model has not seen under this TAG: {}.\nRe-read those lines or retry using the reveal below (now marked seen):{reveal}{more}",
        path.display(),
        unseen
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
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
    let snapshot = format::format_chain_snapshot(display_path, live_text, focus_lines);
    ToolError::Message(format!(
        "{display_path}: {message}\n\nLive snapshot - copy the header and line numbers below to retry:\n{snapshot}"
    ))
}

/// Span endpoints / insert anchors for focused recovery snapshots (not every
/// line inside a large range).
fn focus_anchor_lines(ops: &[Op]) -> Vec<usize> {
    let mut lines = Vec::new();
    for op in ops {
        match op {
            Op::Replace { start, end, .. } | Op::Delete { start, end } => {
                lines.push(*start);
                if *end != *start {
                    lines.push(*end);
                }
            }
            Op::InsertBefore { line, .. } => lines.push(*line),
            Op::InsertAfter {
                line: Some(line), ..
            } => lines.push(*line),
            Op::InsertAfter { line: None, .. } => {}
        }
    }
    lines.sort_unstable();
    lines.dedup();
    lines
}

fn rollback_applied(applied: &[AppliedFile<'_>]) -> Result<(), ToolError> {
    for file in applied.iter().rev() {
        let mut handle = lock_for_rewrite(file.path, file.display_path, " for rollback")?;
        rewrite_locked_file(&mut handle, file.display_path, file.original)?;
    }
    Ok(())
}

fn lock_for_rewrite(
    path: &Path,
    display_path: &str,
    context: &str,
) -> Result<std::fs::File, ToolError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            ToolError::Message(format!("could not open {display_path}{context}: {error}"))
        })?;
    file.lock().map_err(|error| {
        ToolError::Message(format!("could not lock {display_path}{context}: {error}"))
    })?;
    Ok(file)
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

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
