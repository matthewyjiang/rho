//! Apply-patch ToolCard rendering for interactive presentation.
//!
//! Parses a proposed patch document into path/change/stats rows, keeps empty
//! diffs silent, and preserves truncation/path compaction from the shared
//! diff-card helpers.

use rho_tools::{
    tool::compact_display_path,
    tool_card::{DiffCardChange, DiffCardFile, ToolCard, ToolHeader, ToolStatus},
};

use super::{diff_card, kind_card, EmptyDiffState, ToolKind};

pub(super) fn apply_patch_card(
    arguments: &serde_json::Value,
    cwd: &std::path::Path,
    status: ToolStatus,
    trailing_line: rho_tools::apply_patch::ProposedDiffTrailingLine,
) -> ToolCard {
    let Some(input) = arguments.get("input").and_then(serde_json::Value::as_str) else {
        return kind_card(
            status,
            ToolKind::Edit(rho_tools::EditFormat::ApplyPatch),
            ToolHeader::call("apply_patch", None),
        );
    };
    let proposed = rho_tools::apply_patch::proposed_diff_lenient(input, trailing_line);
    let files = proposed
        .files
        .into_iter()
        .map(|file| {
            use rho_tools::apply_patch::ProposedDiffOperation;
            let change = match file.operation {
                ProposedDiffOperation::Delete => DiffCardChange::Delete,
                ProposedDiffOperation::Add | ProposedDiffOperation::Update => {
                    DiffCardChange::Content
                }
            };
            let path = compact_display_path(cwd, &file.display_path);
            let source_path = match (&file.source_path, &file.destination_path) {
                (Some(source), Some(destination)) if source != destination => {
                    Some(compact_display_path(cwd, source))
                }
                _ => None,
            };
            DiffCardFile {
                path,
                source_path,
                change,
                stats: file
                    .added_lines
                    .zip(file.removed_lines)
                    .filter(|(added, removed)| *added > 0 || *removed > 0),
                rows: file.rows,
            }
        })
        .collect::<Vec<_>>();
    diff_card(
        status,
        "apply_patch",
        Vec::new(),
        files,
        EmptyDiffState::Silent,
        proposed.truncated,
    )
}
