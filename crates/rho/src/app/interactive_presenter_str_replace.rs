//! `str_replace` ToolCard rendering for interactive presentation.
//!
//! Projects `old_string`/`new_string` into a proposed diff so streaming and
//! approval cards show the change before the tool runs. Lines shared at the
//! start or end of both strings render as context. When the caller supplies
//! the live file and `old_string` matches exactly once, rows carry real line
//! numbers; otherwise they render without numbers. With `replace_all`, the
//! card shows one replacement plus how many occurrences it applies to.

use rho_tools::tool_card::{
    DiffCardChange, DiffCardFile, DiffRow, DiffRowKind, ToolCard, ToolFact, ToolStatus,
};

use super::{diff_card, display_path, EmptyDiffState};

/// Same cap as the hashline and apply_patch proposed diffs.
const MAX_PROPOSED_DIFF_ROWS: usize = 1_000;

/// Build a proposed-diff card from `str_replace` arguments.
///
/// `live_file` is the current file text when the caller could read it. It
/// supplies line numbers and the `replace_all` occurrence count, and never
/// changes which rows are shown.
pub(super) fn str_replace_card(
    arguments: &serde_json::Value,
    cwd: &std::path::Path,
    status: ToolStatus,
    live_file: Option<&str>,
) -> ToolCard {
    let path = display_path(arguments, cwd);
    let old_string = arguments
        .get("old_string")
        .and_then(serde_json::Value::as_str);
    let replace_all = arguments
        .get("replace_all")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let mut files = Vec::new();
    let mut truncated = false;
    let mut occurrences = None;
    if let (Some(old_string), false) = (old_string, path.is_empty()) {
        let old_string = normalize(old_string);
        let new_string = normalize(
            arguments
                .get("new_string")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
        );
        let match_lines = live_file.map(|file| match_lines(&normalize(file), &old_string));
        let first_line = match match_lines.as_deref() {
            Some([line]) => Some(*line),
            _ => None,
        };
        occurrences = match_lines.map(|lines| lines.len() as u64);
        let proposed = proposed_rows(&old_string, &new_string, first_line);
        truncated = proposed.truncated;
        // Without replace_all the tool fails unless there is exactly one match.
        // With it and no readable file, the total is unknown, so show no stats
        // rather than a single-occurrence count.
        let scale = if replace_all { occurrences } else { Some(1) };
        if proposed.added > 0 || proposed.removed > 0 {
            files.push(DiffCardFile {
                path: path.clone(),
                source_path: None,
                change: DiffCardChange::Content,
                stats: scale
                    .map(|n| (proposed.added * n, proposed.removed * n))
                    .filter(|(added, removed)| *added > 0 || *removed > 0),
                rows: proposed.rows,
            });
        }
    }
    let fallback_paths = (!path.is_empty()).then_some(path).into_iter().collect();
    let mut card = diff_card(
        status,
        rho_tools::EditFormat::StrReplace.tool_name(),
        fallback_paths,
        files,
        EmptyDiffState::Silent,
        truncated,
    );
    if replace_all {
        let text = match occurrences {
            Some(count) if count != 1 => format!("replaces all {count} occurrences"),
            Some(_) => return card,
            None => "replaces every occurrence".into(),
        };
        card.push_fact(ToolFact::Meta { text });
    }
    card
}

struct ProposedRows {
    rows: Vec<DiffRow>,
    added: u64,
    removed: u64,
    truncated: bool,
}

/// Shared lines kept on each side of the change, like a unified diff.
const CONTEXT_LINES: usize = 3;

/// Diff `old` against `new` by trimming shared leading and trailing lines.
/// Only [`CONTEXT_LINES`] of shared context render on each side, so a long
/// shared prefix can never crowd the change out of the row cap.
/// `first_line` is the 1-based line where `old` starts in the live file.
fn proposed_rows(old: &str, new: &str, first_line: Option<u32>) -> ProposedRows {
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(old, new)| old == new)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(old, new)| old == new)
        .count();
    let removed = &old_lines[prefix..old_lines.len() - suffix];
    let added = &new_lines[prefix..new_lines.len() - suffix];
    let line = |offset: usize| {
        first_line.map(|first| first.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX)))
    };
    let skip = || DiffRow::new(DiffRowKind::Skip, None, "⋯");

    let mut rows = Vec::new();
    let leading = prefix.saturating_sub(CONTEXT_LINES);
    if leading > 0 {
        rows.push(skip());
    }
    for (index, text) in old_lines.iter().enumerate().take(prefix).skip(leading) {
        rows.push(DiffRow::new(
            DiffRowKind::Context,
            line(index),
            display(text),
        ));
    }
    for (index, text) in removed.iter().enumerate() {
        rows.push(DiffRow::new(
            DiffRowKind::Removed,
            line(prefix + index),
            display(text),
        ));
    }
    for (index, text) in added.iter().enumerate() {
        rows.push(DiffRow::new(
            DiffRowKind::Added,
            line(prefix + index),
            display(text),
        ));
    }
    let suffix_start = prefix + added.len();
    let trailing = &old_lines[old_lines.len() - suffix..];
    for (index, text) in trailing.iter().take(CONTEXT_LINES).enumerate() {
        rows.push(DiffRow::new(
            DiffRowKind::Context,
            line(suffix_start + index),
            display(text),
        ));
    }
    if trailing.len() > CONTEXT_LINES {
        rows.push(skip());
    }
    let truncated = rows.len() > MAX_PROPOSED_DIFF_ROWS;
    rows.truncate(MAX_PROPOSED_DIFF_ROWS);
    ProposedRows {
        rows,
        added: added.len() as u64,
        removed: removed.len() as u64,
        truncated,
    }
}

/// 1-based start line of each non-overlapping match of `needle` in `file`,
/// the same matches `str_replace` replaces.
fn match_lines(file: &str, needle: &str) -> Vec<u32> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = 1usize;
    let mut scanned = 0;
    for (offset, _) in file.match_indices(needle) {
        line += file[scanned..offset].matches('\n').count();
        scanned = offset;
        lines.push(u32::try_from(line).unwrap_or(u32::MAX));
    }
    lines
}

/// Lines of `text`, each keeping its `\n`. Comparing with terminators makes
/// `foo` and `foo\n` differ without inventing an extra empty line, so adding,
/// removing, or deleting a terminated line counts exactly once.
fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Row text for a line, without its terminator.
fn display(line: &str) -> &str {
    line.strip_suffix('\n').unwrap_or(line)
}

/// `str_replace` treats CRLF, LF, and a bare CR as the same line break, so
/// compare text normalized the same way.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
#[path = "interactive_presenter_str_replace_tests.rs"]
mod tests;
