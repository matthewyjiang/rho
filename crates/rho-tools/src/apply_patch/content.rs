//! Content matching and replacement for update hunks.

use crate::{file_mutation::preferred_line_ending, tool::ToolError};

use super::{parser::UpdateFileChunk, seek_sequence::seek_sequence};

pub(super) fn derive_new_contents(
    original_contents: &str,
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<String, ToolError> {
    let line_ending = preferred_line_ending(original_contents);
    let source_lines = split_source_lines(original_contents);
    let original_lines: Vec<&str> = source_lines.iter().map(|line| line.content).collect();
    let had_trailing_newline = source_lines
        .last()
        .is_some_and(|line| !line.ending.is_empty());
    let replacements = compute_replacements(&original_lines, path, chunks)?;
    Ok(apply_replacements(
        &source_lines,
        &replacements,
        line_ending,
        had_trailing_newline,
    ))
}

struct SourceLine<'a> {
    content: &'a str,
    ending: &'a str,
}

fn split_source_lines(contents: &str) -> Vec<SourceLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = contents.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let ending_len = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
            b'\r' | b'\n' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        lines.push(SourceLine {
            content: &contents[start..index],
            ending: &contents[index..index + ending_len],
        });
        index += ending_len;
        start = index;
    }
    if start < contents.len() {
        lines.push(SourceLine {
            content: &contents[start..],
            ending: "",
        });
    }
    lines
}

fn compute_replacements(
    original_lines: &[&str],
    path: &str,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, ToolError> {
    let mut replacements = Vec::new();
    let mut line_index = 0usize;
    let mut min_next_start = 0usize;

    for chunk in chunks {
        if let Some(ctx_line) = &chunk.change_context {
            let context = std::slice::from_ref(ctx_line);
            if let Some(idx) =
                seek_sequence(original_lines, context, line_index, /*eof*/ false)
            {
                line_index = idx + 1;
            } else if seek_sequence(
                original_lines,
                context,
                /*start*/ 0,
                /*eof*/ false,
            )
            .is_some()
            {
                return Err(ToolError::Message(format!(
                    "patch chunks overlap or apply out of order in {path}"
                )));
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

enum OutputLine<'a> {
    Original(&'a SourceLine<'a>),
    Replacement(&'a str),
}

fn apply_replacements<'a>(
    lines: &'a [SourceLine<'a>],
    replacements: &'a [(usize, usize, Vec<String>)],
    preferred_ending: &str,
    trailing_newline: bool,
) -> String {
    let mut output_lines = Vec::new();
    let mut source_index = 0;
    for (start, old_len, replacement) in replacements {
        output_lines.extend(lines[source_index..*start].iter().map(OutputLine::Original));
        output_lines.extend(
            replacement
                .iter()
                .map(|line| OutputLine::Replacement(line.as_str())),
        );
        source_index = start + old_len;
    }
    output_lines.extend(lines[source_index..].iter().map(OutputLine::Original));

    let mut output = String::new();
    let last = output_lines.len().saturating_sub(1);
    for (index, line) in output_lines.into_iter().enumerate() {
        let has_following_line = index < last;
        match line {
            OutputLine::Original(line) => {
                output.push_str(line.content);
                if has_following_line && line.ending.is_empty() {
                    output.push_str(preferred_ending);
                } else {
                    output.push_str(line.ending);
                }
            }
            OutputLine::Replacement(line) => {
                output.push_str(line);
                if has_following_line || trailing_newline {
                    output.push_str(preferred_ending);
                }
            }
        }
    }
    output
}
