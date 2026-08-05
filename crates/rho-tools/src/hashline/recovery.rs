//! Stale-tag recovery: remap anchors from a stored snapshot onto live text.
//!
//! Recovery fails closed when any anchor disappeared, split, or moved by a
//! non-uniform offset. Callers then surface a live hashline snapshot instead of
//! guessing.

use std::collections::{HashMap, HashSet};

use similar::{ChangeTag, TextDiff};

use super::{
    apply::{apply_ops, ApplyOutcome},
    format::{compute_file_hash, split_content_lines},
    parser::Op,
};

/// Notice when recovery remapped anchors after the live file drifted.
pub(crate) const RECOVERY_REMAP_NOTICE: &str =
    "recovered stale tag by remapping anchors onto the live file";

/// Try to apply `ops` by mapping anchors from `snapshot_text` onto `current`.
pub(crate) fn try_recover(snapshot_text: &str, current: &str, ops: &[Op]) -> Option<ApplyOutcome> {
    let remapped = remap_ops(snapshot_text, current, ops)?;
    let live_tag = compute_file_hash(current);
    apply_ops(current, &live_tag, &remapped).ok()
}

/// Remap every op's original-line anchors from `previous` onto `current`.
///
/// Requires a consistent line offset across all anchors and unambiguous context
/// around each anchor (especially when line text is duplicated).
pub(crate) fn remap_ops(previous: &str, current: &str, ops: &[Op]) -> Option<Vec<Op>> {
    if ops.is_empty() {
        return None;
    }
    let line_map = build_line_map(previous, current);
    if !validate_remapped_anchor_context(previous, current, &line_map, ops) {
        return None;
    }

    let mut offsets = Vec::new();
    let mut map_line = |line: usize| -> Option<usize> {
        let mapped = *line_map.get(&line)?;
        offsets.push(mapped as i32 - line as i32);
        Some(mapped)
    };

    let mut remapped = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            Op::Replace { start, end, body } => {
                let new_start = map_line(*start)?;
                let mut new_end = new_start;
                for line in (*start + 1)..=*end {
                    new_end = map_line(line)?;
                }
                remapped.push(Op::Replace {
                    start: new_start,
                    end: new_end,
                    body: body.clone(),
                });
            }
            Op::Delete { start, end } => {
                let new_start = map_line(*start)?;
                let mut new_end = new_start;
                for line in (*start + 1)..=*end {
                    new_end = map_line(line)?;
                }
                remapped.push(Op::Delete {
                    start: new_start,
                    end: new_end,
                });
            }
            Op::InsertBefore { line, body } => {
                let new_line = map_line(*line)?;
                remapped.push(Op::InsertBefore {
                    line: new_line,
                    body: body.clone(),
                });
            }
            Op::InsertAfter {
                line: Some(line),
                body,
            } => {
                let new_line = map_line(*line)?;
                remapped.push(Op::InsertAfter {
                    line: Some(new_line),
                    body: body.clone(),
                });
            }
            Op::InsertAfter { line: None, body } => {
                remapped.push(Op::InsertAfter {
                    line: None,
                    body: body.clone(),
                });
            }
        }
    }

    if offsets.is_empty() {
        // Only EOF inserts: still valid with zero mapped anchors.
        return Some(remapped);
    }
    let first = offsets[0];
    if !offsets.iter().all(|offset| *offset == first) {
        return None;
    }
    Some(remapped)
}

fn build_line_map(previous: &str, current: &str) -> HashMap<usize, usize> {
    let old_lines = split_content_lines(previous);
    let new_lines = split_content_lines(current);
    let diff = TextDiff::from_slices(&old_lines, &new_lines);
    let mut map = HashMap::new();
    let mut previous_line = 1usize;
    let mut current_line = 1usize;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                map.insert(previous_line, current_line);
                previous_line += 1;
                current_line += 1;
            }
            ChangeTag::Delete => {
                previous_line += 1;
            }
            ChangeTag::Insert => {
                current_line += 1;
            }
        }
    }
    map
}

/// Original-line anchors referenced by ops (every line in a replace/delete span).
pub(crate) fn collect_anchor_lines(ops: &[Op]) -> Vec<usize> {
    let mut lines = Vec::new();
    for op in ops {
        match op {
            Op::Replace { start, end, .. } | Op::Delete { start, end } => {
                lines.extend(*start..=*end);
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

fn collect_duplicated_values(lines: &[&str]) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut duplicated = HashSet::new();
    for line in lines {
        if !seen.insert((*line).to_string()) {
            duplicated.insert((*line).to_string());
        }
    }
    duplicated
}

struct AnchorNeighbors {
    before: Option<usize>,
    after: Option<usize>,
}

fn compute_anchor_neighbors(
    anchor_lines: &HashSet<usize>,
    line_count: usize,
) -> HashMap<usize, AnchorNeighbors> {
    let mut sorted: Vec<usize> = anchor_lines.iter().copied().collect();
    sorted.sort_unstable();
    let mut neighbors = HashMap::new();
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        while j + 1 < sorted.len() && sorted[j + 1] == sorted[j] + 1 {
            j += 1;
        }
        let start = sorted[i];
        let end = sorted[j];
        let before = (start > 1).then_some(start - 1);
        let after = (end < line_count).then_some(end + 1);
        for &line in &sorted[i..=j] {
            neighbors.insert(line, AnchorNeighbors { before, after });
        }
        i = j + 1;
    }
    neighbors
}

fn validate_remapped_anchor_context(
    previous: &str,
    current: &str,
    line_map: &HashMap<usize, usize>,
    ops: &[Op],
) -> bool {
    let previous_lines = split_content_lines(previous);
    let current_lines = split_content_lines(current);
    let anchors: HashSet<usize> = collect_anchor_lines(ops).into_iter().collect();
    if anchors.is_empty() {
        return true;
    }
    let duplicated_previous = collect_duplicated_values(&previous_lines);
    let duplicated_current = collect_duplicated_values(&current_lines);
    let neighbors = compute_anchor_neighbors(&anchors, previous_lines.len());

    for (line, neighbor) in neighbors {
        let Some(&mapped) = line_map.get(&line) else {
            return false;
        };
        if mapped == 0 || mapped > current_lines.len() {
            return false;
        }
        let prev_text = previous_lines[line - 1];
        let curr_text = current_lines[mapped - 1];
        if prev_text != curr_text {
            return false;
        }
        let is_dup =
            duplicated_previous.contains(prev_text) || duplicated_current.contains(curr_text);
        if is_dup {
            if !validate_duplicate_anchor_context(line, mapped, &neighbor, line_map) {
                return false;
            }
        } else if !validate_unique_anchor_context(line, mapped, &neighbor, line_map) {
            return false;
        }
    }
    true
}

fn validate_duplicate_anchor_context(
    line: usize,
    mapped: usize,
    neighbors: &AnchorNeighbors,
    line_map: &HashMap<usize, usize>,
) -> bool {
    let mut checked = false;
    if let Some(before) = neighbors.before {
        checked = true;
        let Some(&mapped_before) = line_map.get(&before) else {
            return false;
        };
        if mapped_before + (line - before) != mapped {
            return false;
        }
    }
    if let Some(after) = neighbors.after {
        checked = true;
        let Some(&mapped_after) = line_map.get(&after) else {
            return false;
        };
        if mapped + (after - line) != mapped_after {
            return false;
        }
    }
    checked
}

fn validate_unique_anchor_context(
    line: usize,
    mapped: usize,
    neighbors: &AnchorNeighbors,
    line_map: &HashMap<usize, usize>,
) -> bool {
    let offset = mapped as i32 - line as i32;
    if let Some(after) = neighbors.after {
        if line_map.get(&after).copied() == Some((after as i32 + offset) as usize) {
            return true;
        }
    }
    if let Some(before) = neighbors.before {
        if line_map.get(&before).copied() == Some((before as i32 + offset) as usize) {
            return true;
        }
    }
    neighbors.before.is_none() && neighbors.after.is_none()
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
