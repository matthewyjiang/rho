//! What a call tells hosts besides its title and output: its ACP kind, and
//! the files it touches, at a line when the call knows one, for hosts that
//! follow the agent through the workspace.

use std::path::Path;

use agent_client_protocol::schema::v1::{ToolCallLocation, ToolKind};
use rho_sdk::tool::{OperationKind, ToolFileChange, ToolMetadata, ToolOutput};
use rho_tools::{
    apply_patch::{parse_patch, Hunk},
    EditFormat,
};
use serde::Deserialize;
use serde_json::Value;

use crate::tools::{plan_exit, todo};

/// The read tool, whose `offset` argument is the first line it shows.
const READ_FILE_TOOL: &str = "read_file";

/// Most grep-matched files a finished call names. Zed opens a buffer for
/// every location; its own grep tool sends at most 20 matches per call
/// (`RESULTS_PER_PAGE`), while Rho's grep can return up to 1000 files.
const MAX_GREP_LOCATIONS: usize = 20;

/// A started call's kind, from its operation. Rho's checklist and plan-exit
/// tools declare none, so their names decide, and a patch that only deletes
/// or only moves files says so.
pub(super) fn kind(name: &str, arguments: Option<&Value>, metadata: &ToolMetadata) -> ToolKind {
    match metadata.operation_kind() {
        Some(OperationKind::Read) => ToolKind::Read,
        Some(OperationKind::Search) => ToolKind::Search,
        Some(OperationKind::Write) => match (EditFormat::from_tool_name(name), arguments) {
            (Some(EditFormat::ApplyPatch), Some(arguments)) => patch_kind(arguments),
            _ => ToolKind::Edit,
        },
        Some(OperationKind::Execute) => ToolKind::Execute,
        Some(OperationKind::Network) => ToolKind::Fetch,
        Some(OperationKind::Other(_)) | None => match name {
            todo::TOOL_NAME => ToolKind::Think,
            plan_exit::TOOL_NAME => ToolKind::SwitchMode,
            _ => ToolKind::Other,
        },
        // OperationKind is non_exhaustive; an unknown operation says nothing.
        _ => ToolKind::Other,
    }
}

/// `delete` when every file section of the patch deletes a file, `move` when
/// every one moves a file, else `edit`.
fn patch_kind(arguments: &Value) -> ToolKind {
    let Some(Ok(hunks)) = arguments
        .get("input")
        .and_then(Value::as_str)
        .map(parse_patch)
    else {
        return ToolKind::Edit;
    };
    if hunks.iter().all(|hunk| matches!(hunk, Hunk::Delete { .. })) {
        ToolKind::Delete
    } else if hunks.iter().all(|hunk| {
        matches!(
            hunk,
            Hunk::Update {
                move_path: Some(_),
                ..
            }
        )
    }) {
        ToolKind::Move
    } else {
        ToolKind::Edit
    }
}

/// The paths a started call names, absolute against the session directory
/// as ACP requires. A read names the line it starts at.
///
/// Lines are 0-based rows, as Zed reads them and as its own tools send them;
/// ACP does not say.
pub(super) fn started_locations(
    name: &str,
    arguments: Option<&Value>,
    metadata: &ToolMetadata,
    cwd: &Path,
) -> Vec<ToolCallLocation> {
    let line = arguments
        .filter(|_| name == READ_FILE_TOOL)
        .and_then(|arguments| arguments.get("offset")?.as_u64())
        .and_then(row);
    metadata
        .affected_paths()
        .iter()
        .map(|path| ToolCallLocation::new(cwd.join(path)).line(line))
        .collect()
}

/// Where a finished call's output points: each file it changed, at the first
/// changed line, or the first [`MAX_GREP_LOCATIONS`] files a grep matched,
/// at the first match. `None` when the output names no such files, so the
/// started locations stand.
pub(super) fn finished_locations(output: &ToolOutput, cwd: &Path) -> Option<Vec<ToolCallLocation>> {
    let metadata = output.presentation();
    let locations = if !metadata.file_changes().is_empty() {
        metadata
            .file_changes()
            .iter()
            .map(|change| ToolCallLocation::new(cwd.join(change.path())).line(changed_line(change)))
            .collect()
    } else if metadata.operation_kind() == Some(&OperationKind::Search) {
        // Only grep's result has this shape; glob lists `paths` instead, so
        // it fails to parse and keeps its started locations.
        GrepHits::deserialize(output.structured_content()?)
            .ok()?
            .files
            .into_iter()
            .take(MAX_GREP_LOCATIONS)
            .map(|file| {
                let line = file.lines.first().and_then(|line| row(line.line));
                ToolCallLocation::new(cwd.join(file.path)).line(line)
            })
            .collect()
    } else {
        return None;
    };
    Some(locations).filter(|locations: &Vec<_>| !locations.is_empty())
}

/// The row where a changed file first differs from its old text. `None`
/// for a new file, a file left empty, or text that did not change.
fn changed_line(change: &ToolFileChange) -> Option<u32> {
    let mut old_lines = change.old_text()?.lines();
    let mut rows = 0usize;
    for (index, new_line) in change.new_text().lines().enumerate() {
        if old_lines.next() != Some(new_line) {
            return u32::try_from(index).ok();
        }
        rows = index + 1;
    }
    // Lines were cut from the end: point at what is now the last row.
    (rows > 0 && old_lines.next().is_some())
        .then(|| u32::try_from(rows - 1).ok())
        .flatten()
}

/// The 0-based row of a tool's 1-based line number.
fn row(line: u64) -> Option<u32> {
    u32::try_from(line.checked_sub(1)?).ok()
}

/// The part of grep's structured result (its published output schema) that
/// locates hits: each matched file, workspace-relative, with the lines shown.
#[derive(Deserialize)]
struct GrepHits {
    files: Vec<GrepFile>,
}

#[derive(Deserialize)]
struct GrepFile {
    path: std::path::PathBuf,
    #[serde(default)]
    lines: Vec<GrepLine>,
}

#[derive(Deserialize)]
struct GrepLine {
    /// 1-based, as grep prints it.
    line: u64,
}

#[cfg(test)]
#[path = "events_tools_tests.rs"]
mod tests;
