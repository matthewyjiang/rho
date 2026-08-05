//! Line-anchored multi-hunk edit tool (`edit`) with snapshot tags.
//!
//! `read_file` / `write_file` mint `[path#TAG]` snapshots. `grep` content mode
//! mints headers + line numbers for anchors (match text is preview only). `edit`
//! applies a compact PUT/CUT document against those original line numbers and
//! rejects stale tags. Failures leave the file untouched and return a bounded
//! live snapshot to copy.

mod apply;
mod format;
mod parser;
mod proposed;

use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::json;

use crate::{diff::unified_diff, file_mutation::FileMutationOutcome, tool::*};

use apply::apply_ops;
use parser::{Op, Section};

pub(crate) use format::{
    format_chain_snapshot, format_hashline_view, format_header, format_post_edit_preview,
    split_content_lines,
};
pub(crate) use parser::parse_hashline;
pub use proposed::{planned_edit, proposed_edit, proposed_sections, EditPreview, ProposedSection};

pub(crate) use format::compute_file_hash;

/// Replace/delete span size that marks a structural edit in the post-edit footer.
const STRUCTURAL_EDIT_SPAN_LINES: usize = 40;

pub(crate) struct Edit;

/// Operational contract only. Chaining policy and dialect tips live in the system
/// prompt / docs so this schema string does not drift as a third essay.
const TOOL_DESCRIPTION: &str = r#"Multi-hunk line-anchored edits to existing UTF-8 files.

Requires a fresh `[path#TAG]` from `read_file`, `grep` (content mode TAG + line numbers only), `write_file` snapshot, a prior non-structural `edit` preview, or a failed `edit` live snapshot. Never invent a TAG. Prefer `write_file` to create or fully rewrite a file.

Document:
[path#TAG]
PUT N:
+replacement
PUT N.=M:
+range body
PUT <N: / PUT >N: / PUT >$:
+insert
CUT N.=M

Locators: `PUT 12:` (single line; never `PUT 12.:`), `PUT 12.=15:` (inclusive range; also `12-15` / `12..15`), inserts as above, `CUT` without a colon. Body rows under `:` headers must start with `+`. PUT needs ≥1 body row; use CUT to delete. Line numbers are original snapshot lines and do not shift mid-document. One section per path; do not stack two `edit` calls on the same path in one batch. Stale TAG, overlap, out-of-range, and mid-edit changes fail closed with a live snapshot to copy."#;

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
                        "description": "Hashline document: one or more [path#TAG] sections with PUT/CUT ops. Copy each TAG from a fresh snapshot; never invent tags."
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
        // App-tool harness for unit tests only. Production runs go through the
        // SDK EditTool (workspace resolve, capabilities, resources, revalidate).
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
        let outcome = apply_prepared_sections(sections, ctx.max_output_bytes).await?;
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
///
/// Path uniqueness for the mutation is enforced here for every caller (App
/// harness and SDK execute). The SDK prepare path may also reject duplicates
/// earlier as `InvalidArguments` so authorization never starts for a malformed
/// multi-claim document — that is a fail-fast gate, not a second owner of the
/// write-time invariant.
pub(crate) async fn apply_prepared_sections(
    sections: Vec<PreparedSection>,
    max_output_bytes: usize,
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

    tokio::task::spawn_blocking(move || apply_sections_locked(sections, max_output_bytes))
        .await
        .map_err(|error| ToolError::Message(format!("hashline edit task failed: {error}")))?
}

/// One file's planned rewrite, before anything is written.
struct PlannedFile {
    path: PathBuf,
    display_path: String,
    original: String,
    outcome: apply::ApplyOutcome,
    anchor_lines: Vec<usize>,
    ops_summary: String,
    structural: bool,
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
    let mut parts = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            Op::Replace { start, end, body } => {
                let removed = end.saturating_sub(*start).saturating_add(1);
                if start == end {
                    parts.push(format!("PUT {start}: (1 → {} line(s))", body.len()));
                } else {
                    parts.push(format!(
                        "PUT {start}.={end}: ({removed} → {} line(s))",
                        body.len()
                    ));
                }
            }
            Op::Delete { start, end } => {
                let removed = end.saturating_sub(*start).saturating_add(1);
                if start == end {
                    parts.push(format!("CUT {start} ({removed} line)"));
                } else {
                    parts.push(format!("CUT {start}.={end} ({removed} lines)"));
                }
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

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
