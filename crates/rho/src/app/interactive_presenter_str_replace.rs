//! `str_replace` ToolCard rendering for interactive presentation.
//!
//! Streaming and approval cards show the change before the tool runs. When
//! the live file is readable and the call would apply, the card is the exact
//! diff of the file before and after the replacement, so line numbers and
//! stats match the result card. Otherwise it projects `old_string` against
//! `new_string` without line numbers. With `replace_all`, the card also says
//! how many occurrences it applies to.

use rho_tools::tool_card::{
    parse_unified_diff, DiffCardChange, DiffCardFile, DiffRow, DiffRowKind, ToolCard, ToolFact,
    ToolStatus,
};

use super::{diff_card, display_path, EmptyDiffState};

/// Same cap as the hashline and apply_patch proposed diffs.
const MAX_PROPOSED_DIFF_ROWS: usize = 1_000;
/// Same cap as the result diff `str_replace` itself records (old plus new
/// bytes), so the exact preview never costs more than the finished card.
const MAX_EXACT_DIFF_BYTES: usize = 128 * 1024;
/// Shared lines kept on each side of a change, like a unified diff.
const CONTEXT_LINES: usize = 3;

/// Build a proposed-diff card from `str_replace` arguments.
///
/// `live_file` is the current file text when the caller could read it.
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
        let live_file = live_file.map(normalize);
        occurrences = live_file
            .as_deref()
            .map(|file| count_matches(file, &old_string));
        let exact = live_file
            .as_deref()
            .and_then(|file| exact_diff(file, &old_string, &new_string, replace_all, &path));
        files = match exact {
            Some(files) => files,
            None => projected_diff(&old_string, &new_string, replace_all, &path)
                .into_iter()
                .collect(),
        };
        for file in &mut files {
            truncated |= file.rows.len() > MAX_PROPOSED_DIFF_ROWS;
            file.rows.truncate(MAX_PROPOSED_DIFF_ROWS);
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

/// Diff of `file` before and after the replacement, when the tool would
/// apply it: at least one match, exactly one unless `replace_all`, and a file
/// small enough to diff before and after. Matches are non-overlapping, like
/// the tool's. The result size is checked before it is built, so a bulk
/// replacement cannot allocate past the cap.
fn exact_diff(
    file: &str,
    old: &str,
    new: &str,
    replace_all: bool,
    path: &str,
) -> Option<Vec<DiffCardFile>> {
    if old.is_empty() || file.len() > MAX_EXACT_DIFF_BYTES {
        return None;
    }
    let count = match count_matches(file, old) {
        0 => return None,
        count if count > 1 && !replace_all => return None,
        count => usize::try_from(count).ok()?,
    };
    let updated_len =
        (file.len() - count * old.len()).checked_add(count.checked_mul(new.len())?)?;
    if file.len().checked_add(updated_len)? > MAX_EXACT_DIFF_BYTES {
        return None;
    }
    let updated = file.replace(old, new);
    let diff = similar::TextDiff::from_lines(file, &updated)
        .unified_diff()
        .context_radius(CONTEXT_LINES)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();
    Some(
        parse_unified_diff(&diff)
            .into_iter()
            .map(|parsed| DiffCardFile {
                path: path.to_string(),
                ..DiffCardFile::from(parsed)
            })
            .collect(),
    )
}

/// Projection of `old` against `new` alone, for when the live file cannot
/// say where or how often the replacement lands. Rows carry no line numbers,
/// and `replace_all` shows no stats because the total is unknown.
fn projected_diff(old: &str, new: &str, replace_all: bool, path: &str) -> Option<DiffCardFile> {
    let proposed = proposed_rows(old, new);
    (proposed.added > 0 || proposed.removed > 0).then(|| DiffCardFile {
        path: path.to_string(),
        source_path: None,
        change: DiffCardChange::Content,
        stats: (!replace_all).then_some((proposed.added, proposed.removed)),
        rows: proposed.rows,
    })
}

fn count_matches(file: &str, needle: &str) -> u64 {
    if needle.is_empty() {
        0
    } else {
        file.matches(needle).count() as u64
    }
}

struct ProposedRows {
    rows: Vec<DiffRow>,
    added: u64,
    removed: u64,
}

/// Diff `old` against `new` by trimming shared leading and trailing lines.
/// Only [`CONTEXT_LINES`] of shared context render on each side, so a long
/// shared prefix can never crowd the change out of the row cap.
fn proposed_rows(old: &str, new: &str) -> ProposedRows {
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
    let row = |kind, text: &&str| DiffRow::new(kind, None, display(text));
    let skip = || DiffRow::new(DiffRowKind::Skip, None, "⋯");

    let mut rows = Vec::new();
    let leading = prefix.saturating_sub(CONTEXT_LINES);
    if leading > 0 {
        rows.push(skip());
    }
    rows.extend(
        old_lines[leading..prefix]
            .iter()
            .map(|text| row(DiffRowKind::Context, text)),
    );
    rows.extend(removed.iter().map(|text| row(DiffRowKind::Removed, text)));
    rows.extend(added.iter().map(|text| row(DiffRowKind::Added, text)));
    let trailing = &old_lines[old_lines.len() - suffix..];
    rows.extend(
        trailing
            .iter()
            .take(CONTEXT_LINES)
            .map(|text| row(DiffRowKind::Context, text)),
    );
    if trailing.len() > CONTEXT_LINES {
        rows.push(skip());
    }
    ProposedRows {
        rows,
        added: added.len() as u64,
        removed: removed.len() as u64,
    }
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
