//! Hashline edit tool: line-anchored multi-hunk edits with snapshot tags.
//!
//! `read_file` returns `[path#TAG]` plus `N:line` rows. `hashline_edit` applies
//! a compact PUT/CUT document against those original line numbers and rejects
//! stale tags before writing.

mod apply;
mod format;
mod parser;

use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::json;

use crate::{diff::unified_diff, tool::*, write_file::FileMutationOutcome};

pub use apply::{apply_ops, ApplyOutcome};
pub use format::{
    compute_file_hash, format_hashline_view, format_header, format_numbered_line, FILE_HASH_LENGTH,
};
pub use parser::{parse_hashline, proposed_sections, Op, ProposedSection, Section};

pub struct HashlineEdit;

const TOOL_DESCRIPTION: &str = r#"Use `hashline_edit` for multi-hunk edits to existing UTF-8 files with line anchors from `read_file`. Prefer this when you already read the file and have a `[path#TAG]` header. Prefer `edit_file` for one exact string replace when you did not take a hashline read. Prefer `write_file` to create or fully rewrite a file. Prefer `apply_patch` for Codex-style multi-file patches that add or delete files.

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
- TAG is the 8-hex snapshot from the latest `read_file` header and is required
- Line numbers name ORIGINAL lines from that read; they do not shift mid-document
- Body rows under PUT headers that end with `:` must start with `+` (use `+` alone for a blank line)
- Ranges are inclusive (`PUT 3.=5:` touches original lines 3 through 5)
- Single-line replace may use `PUT N:` as shorthand for `PUT N.=N:`
- Stale TAG or out-of-range lines fail closed with no write
- An insert anchored inside a range another op replaces or deletes is rejected; anchor it outside
- TAG ignores trailing whitespace, so whitespace-only changes keep a read valid
- Block ops (`N*`), registers (`@name`), REM, and MV are not supported yet
- Create files with `write_file`; do not use hashline_edit to create paths"#;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    input: String,
}

#[async_trait::async_trait]
impl Tool for HashlineEdit {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "hashline_edit".into(),
            description: TOOL_DESCRIPTION.into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string",
                        "description": "Hashline document with one or more [path#TAG] sections and PUT/CUT ops."
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
    outcome: ApplyOutcome,
    op_count: usize,
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
    for prepared in &sections {
        let display_path = &prepared.display_path;
        let original = std::fs::read_to_string(&prepared.path).map_err(|error| {
            ToolError::Message(format!("could not read {display_path}: {error}"))
        })?;
        let outcome = apply_ops(&original, &prepared.section.tag, &prepared.section.ops)
            .map_err(|error| ToolError::Message(format!("{display_path}: {error}")))?;
        planned.push(PlannedFile {
            path: prepared.path.clone(),
            display_path: display_path.clone(),
            original,
            outcome,
            op_count: prepared.section.ops.len(),
        });
    }

    let mut applied: Vec<AppliedFile<'_>> = Vec::new();
    let mut summaries = Vec::new();
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
        summaries.push(format!(
            "edited {} (tag {} -> {}); {} op(s)",
            file.display_path, file.outcome.old_tag, file.outcome.new_tag, file.op_count
        ));
        diffs.push(unified_diff(
            &file.original,
            &file.outcome.text,
            &file.display_path,
            /*created*/ false,
        ));
    }

    let diff = diffs.join("\n");
    let content = truncate(
        format!("{}\n\n{diff}", summaries.join("\n")),
        max_output_bytes,
    );
    Ok(FileMutationOutcome {
        content,
        display_path: planned
            .first()
            .map(|file| file.display_path.clone())
            .unwrap_or_default(),
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
        return Err(ToolError::Message(format!(
            "{display_path} changed during hashline_edit; re-read and retry"
        )));
    }
    rewrite_locked_file(&mut handle, display_path, &file.outcome.text)
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
