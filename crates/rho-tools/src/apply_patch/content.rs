//! Content matching and replacement for update hunks.

use crate::{
    file_mutation::{normalize_newlines, preferred_line_ending},
    tool::ToolError,
};

use super::{parser::UpdateFileChunk, seek_sequence::seek_sequence};

pub(super) fn derive_new_contents(
    original_contents: &str,
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<String, ToolError> {
    let line_ending = preferred_line_ending(original_contents);
    let normalized_original = normalize_newlines(original_contents);
    let had_trailing_newline = normalized_original.ends_with('\n');
    let original_lines = split_lines(&normalized_original);
    let replacements = compute_replacements(&original_lines, path, chunks)?;
    let new_lines = apply_replacements(original_lines, &replacements);
    Ok(join_lines(&new_lines, had_trailing_newline).replace('\n', line_ending))
}

fn split_lines(contents: &str) -> Vec<String> {
    if contents.is_empty() {
        return Vec::new();
    }
    let body = contents.strip_suffix('\n').unwrap_or(contents);
    if body.is_empty() {
        return vec![String::new()];
    }
    body.split('\n').map(String::from).collect()
}

fn join_lines(lines: &[String], trailing_newline: bool) -> String {
    let mut out = lines.join("\n");
    if trailing_newline {
        out.push('\n');
    }
    out
}

fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, ToolError> {
    let mut replacements = Vec::new();
    let mut line_index = 0usize;
    let mut min_next_start = 0usize;

    for chunk in chunks {
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                /*eof*/ false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(ToolError::Message(format!(
                    "Failed to find context '{ctx_line}' in {path}"
                )));
            }
        }

        if chunk.old_lines.is_empty() {
            if line_index < min_next_start {
                return Err(ToolError::Message(format!(
                    "patch chunks overlap or apply out of order in {path}"
                )));
            }
            replacements.push((line_index, 0, chunk.new_lines.clone()));
            min_next_start = line_index;
            continue;
        }

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        if let Some(start_idx) = found {
            if start_idx < min_next_start {
                return Err(ToolError::Message(format!(
                    "patch chunks overlap or apply out of order in {path}"
                )));
            }
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            min_next_start = start_idx + pattern.len();
            line_index = min_next_start;
        } else {
            return Err(ToolError::Message(format!(
                "Failed to find expected lines in {path}:\n{}",
                chunk.old_lines.join("\n")
            )));
        }
    }

    Ok(replacements)
}

fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        let old_len = *old_len;
        let end = (start_idx + old_len).min(lines.len());
        if start_idx <= lines.len() {
            lines.splice(start_idx..end, new_segment.iter().cloned());
        }
    }
    lines
}
