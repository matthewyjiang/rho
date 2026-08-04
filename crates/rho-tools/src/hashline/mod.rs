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
pub use parser::{parse_hashline, section_paths_lenient, Op, Section};

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
- TAG is the 4-hex snapshot from the latest `read_file` header and is required
- Line numbers name ORIGINAL lines from that read; they do not shift mid-document
- Body rows under PUT headers that end with `:` must start with `+` (use `+` alone for a blank line)
- Ranges are inclusive (`PUT 3.=5:` touches original lines 3 through 5)
- Single-line replace may use `PUT N:` as shorthand for `PUT N.=N:`
- Stale TAG or out-of-range lines fail closed with no write
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
        let cwd = ctx.cwd.clone();
        let outcome = apply_hashline_document(
            &args.input,
            |path| Ok(resolve_path(&cwd, path)),
            |path| compact_display_path(&cwd, path),
            ctx.max_output_bytes,
        )
        .await?;
        Ok(ToolResult {
            id,
            ok: true,
            content: outcome.content,
        })
    }
}

/// Apply a multi-section hashline document to the workspace.
pub(crate) async fn apply_hashline_document<Resolve, Display>(
    input: &str,
    resolve: Resolve,
    display: Display,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError>
where
    Resolve: Fn(&str) -> Result<PathBuf, ToolError> + Send + Sync,
    Display: Fn(&str) -> String + Send + Sync,
{
    let sections = parse_hashline(input).map_err(|error| ToolError::Message(error.to_string()))?;
    let mut resolved = Vec::with_capacity(sections.len());
    let mut seen = BTreeMap::<PathBuf, String>::new();
    for section in sections {
        let path = resolve(&section.path)?;
        if let Some(prior) = seen.insert(path.clone(), section.path.clone()) {
            return Err(ToolError::Message(format!(
                "hashline document claims path '{}' more than once (also as '{prior}')",
                section.path
            )));
        }
        let display_path = display(&section.path);
        resolved.push((section, path, display_path));
    }

    tokio::task::spawn_blocking(move || apply_hashline_document_locked(resolved, max_output_bytes))
        .await
        .map_err(|error| ToolError::Message(format!("hashline edit task failed: {error}")))?
}

fn apply_hashline_document_locked(
    sections: Vec<(Section, PathBuf, String)>,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError> {
    // Plan every file first so a later section failure cannot leave earlier
    // writes applied.
    let mut planned = Vec::with_capacity(sections.len());
    for (section, path, display_path) in &sections {
        let original = std::fs::read_to_string(path).map_err(|error| {
            ToolError::Message(format!("could not read {display_path}: {error}"))
        })?;
        let outcome = apply_ops(&original, &section.tag, &section.ops)
            .map_err(|error| ToolError::Message(format!("{display_path}: {error}")))?;
        planned.push((
            path.clone(),
            display_path.clone(),
            original,
            outcome,
            section.ops.len(),
        ));
    }

    // Revalidate every target before the first write so a mid-flight change cannot
    // partially apply a multi-file document.
    for (path, display_path, original, _, _) in &planned {
        let live = std::fs::read_to_string(path).map_err(|error| {
            ToolError::Message(format!("could not revalidate {display_path}: {error}"))
        })?;
        if live != *original {
            return Err(ToolError::Message(format!(
                "{display_path} changed during hashline_edit; re-read and retry"
            )));
        }
    }

    let mut applied: Vec<(&Path, &str, &str)> = Vec::new();
    let mut summaries = Vec::new();
    let mut diffs = Vec::new();
    let mut first_display = None;
    for (path, display_path, original, outcome, op_count) in &planned {
        if let Err(error) = commit_planned_file(path, display_path, original, &outcome.text) {
            let mut message = error.to_string();
            if let Err(rollback_error) = rollback_applied(&applied) {
                message = format!("{message}; rollback also failed: {rollback_error}");
            } else if !applied.is_empty() {
                message = format!("{message}; applied changes were rolled back");
            }
            return Err(ToolError::Message(message));
        }
        applied.push((path.as_path(), display_path.as_str(), original.as_str()));
        let diff = unified_diff(
            original,
            &outcome.text,
            display_path,
            /*created*/ false,
        );
        summaries.push(format!(
            "edited {display_path} (tag {} -> {}); {op_count} op(s)",
            outcome.old_tag, outcome.new_tag
        ));
        diffs.push(diff);
        if first_display.is_none() {
            first_display = Some(display_path.clone());
        }
    }

    let content = truncate(
        format!("{}\n\n{}", summaries.join("\n"), diffs.join("\n")),
        max_output_bytes,
    );
    Ok(FileMutationOutcome {
        content,
        display_path: first_display.unwrap_or_default(),
        diff: diffs.join("\n"),
    })
}

fn commit_planned_file(
    path: &Path,
    display_path: &str,
    expected_original: &str,
    new_text: &str,
) -> Result<(), ToolError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| ToolError::Message(format!("could not open {display_path}: {error}")))?;
    file.lock()
        .map_err(|error| ToolError::Message(format!("could not lock {display_path}: {error}")))?;
    let mut live = String::new();
    file.read_to_string(&mut live)
        .map_err(|error| ToolError::Message(format!("could not read {display_path}: {error}")))?;
    if live != expected_original {
        return Err(ToolError::Message(format!(
            "{display_path} changed during hashline_edit; re-read and retry"
        )));
    }
    rewrite_locked_file(&mut file, display_path, new_text)
}

fn rollback_applied(applied: &[(&Path, &str, &str)]) -> Result<(), ToolError> {
    for (path, display_path, original) in applied.iter().rev() {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| {
                ToolError::Message(format!(
                    "could not open {display_path} for rollback: {error}"
                ))
            })?;
        file.lock().map_err(|error| {
            ToolError::Message(format!(
                "could not lock {display_path} for rollback: {error}"
            ))
        })?;
        rewrite_locked_file(&mut file, display_path, original)?;
    }
    Ok(())
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
