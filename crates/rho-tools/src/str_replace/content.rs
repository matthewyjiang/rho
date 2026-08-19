//! Exact string matching and locked file replacement for `str_replace`.

use std::{io::Read, ops::Range, path::Path};

use crate::{
    diff::unified_diff,
    file_mutation::{
        lock_for_rewrite, locked_path_identity_matches, normalize_newlines, preferred_line_ending,
        rewrite_locked_file, FileMutationOutcome,
    },
    tool::*,
};

/// Apply one string replacement to an existing file under an exclusive lock.
pub(crate) async fn str_replace_content(
    path: &Path,
    display_path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError> {
    validate_edit_args(old_string, new_string)?;

    let path = path.to_path_buf();
    let display_path = display_path.to_string();
    let old_string = old_string.to_string();
    let new_string = new_string.to_string();
    tokio::task::spawn_blocking(move || {
        str_replace_content_locked(
            &path,
            &display_path,
            &old_string,
            &new_string,
            replace_all,
            max_output_bytes,
        )
    })
    .await
    .map_err(|error| ToolError::Message(format!("edit task failed: {error}")))?
}

pub(super) fn validate_edit_args(old_string: &str, new_string: &str) -> Result<(), ToolError> {
    if old_string.is_empty() {
        return Err(ToolError::Message("old_string must not be empty".into()));
    }
    if normalize_newlines(old_string) == normalize_newlines(new_string) {
        return Err(ToolError::Message(
            "old_string and new_string are identical after newline normalization; nothing to change".into(),
        ));
    }
    Ok(())
}

fn str_replace_content_locked(
    path: &Path,
    display_path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError> {
    let mut file = lock_for_rewrite(path, display_path, "")?;
    if !locked_path_identity_matches(&file, path, display_path)? {
        return Err(ToolError::Message(format!(
            "edit {display_path} failed: path changed after validation"
        )));
    }

    let mut original = String::new();
    file.read_to_string(&mut original)
        .map_err(|error| ToolError::Message(format!("could not read {display_path}: {error}")))?;

    let spans = replacement_spans(&original, old_string);
    validate_match_count(display_path, spans.len(), replace_all)?;
    let replacement = match_file_eol(&original, new_string);
    let updated = replace_spans(&original, &spans, &replacement);

    rewrite_locked_file(
        &mut file,
        display_path,
        /*original*/ &original,
        /*updated*/ &updated,
    )?;

    let diff = unified_diff(&original, &updated, display_path, /*created*/ false);
    let replaced = spans.len();
    let snapshot = crate::text_view::format_chain_snapshot(display_path, &updated, &[]);
    Ok(FileMutationOutcome {
        content: truncate(
            format!("edited {display_path}; replaced {replaced} occurrence(s)\n\n{snapshot}"),
            max_output_bytes,
        ),
        display_paths: vec![display_path.to_string()],
        diff,
    })
}

fn validate_match_count(
    display_path: &str,
    actual: usize,
    replace_all: bool,
) -> Result<(), ToolError> {
    if replace_all {
        if actual == 0 {
            return Err(ToolError::Message(format!(
                "edit {display_path} failed: missing match: found 0 occurrence(s)"
            )));
        }
        return Ok(());
    }
    if actual == 1 {
        return Ok(());
    }
    let reason = if actual == 0 {
        "missing match"
    } else {
        "ambiguous match"
    };
    Err(ToolError::Message(format!(
        "edit {display_path} failed: {reason}: found {actual} occurrence(s), expected 1"
    )))
}

fn replacement_spans(content: &str, old_string: &str) -> Vec<Range<usize>> {
    let normalized_content = normalize_newlines(content);
    let normalized_old = normalize_newlines(old_string);
    let normalized_spans: Vec<_> = normalized_content
        .match_indices(&normalized_old)
        .map(|(start, matched)| start..start + matched.len())
        .collect();
    let boundaries: Vec<_> = normalized_spans
        .iter()
        .flat_map(|span| [span.start, span.end])
        .collect();
    let source_offsets = source_offsets_for_normalized_boundaries(content, &boundaries);
    source_offsets
        .chunks_exact(2)
        .map(|chunk| chunk[0]..chunk[1])
        .collect()
}

fn source_offsets_for_normalized_boundaries(source: &str, boundaries: &[usize]) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(boundaries.len());
    let mut boundary = 0;
    let mut normalized_offset = 0;
    let mut chars = source.char_indices().peekable();
    while let Some((source_start, ch)) = chars.next() {
        while boundaries.get(boundary) == Some(&normalized_offset) {
            offsets.push(source_start);
            boundary += 1;
        }

        let (source_end, normalized_len) = if ch == '\r' {
            if matches!(chars.peek(), Some((_, '\n'))) {
                chars.next();
                (source_start + 2, 1)
            } else {
                (source_start + 1, 1)
            }
        } else {
            (source_start + ch.len_utf8(), ch.len_utf8())
        };
        normalized_offset += normalized_len;
        while boundaries.get(boundary) == Some(&normalized_offset) {
            offsets.push(source_end);
            boundary += 1;
        }
    }
    while boundaries.get(boundary) == Some(&normalized_offset) {
        offsets.push(source.len());
        boundary += 1;
    }
    debug_assert_eq!(boundary, boundaries.len());
    offsets
}

fn replace_spans(content: &str, spans: &[Range<usize>], new_string: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut last = 0;
    for span in spans {
        output.push_str(&content[last..span.start]);
        output.push_str(new_string);
        last = span.end;
    }
    output.push_str(&content[last..]);
    output
}

fn match_file_eol(content: &str, new_string: &str) -> String {
    let eol = preferred_line_ending(content);
    normalize_newlines(new_string).replace('\n', eol)
}
