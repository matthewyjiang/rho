//! Pure apply of parsed hashline ops onto a text body.

use super::{
    format::{compute_file_hash, detect_eol, has_trailing_newline, split_content_lines},
    parser::Op,
};

/// Result of applying ops to one file body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub text: String,
    pub old_tag: String,
    pub new_tag: String,
}

/// Apply `ops` to `original` after verifying `expected_tag`.
///
/// All ops address **original** line numbers from the tagged snapshot. Overlapping
/// destructive spans are rejected. Inserts may share an anchor line with each
/// other and run in document order at that anchor.
pub fn apply_ops(original: &str, expected_tag: &str, ops: &[Op]) -> Result<ApplyOutcome, String> {
    let old_tag = compute_file_hash(original);
    if !old_tag.eq_ignore_ascii_case(expected_tag) {
        return Err(format!(
            "hashline tag mismatch: expected {expected_tag}, live file is {old_tag}. Re-read the file and retry with the new tag and line numbers."
        ));
    }
    if ops.is_empty() {
        return Err("hashline section has no operations".into());
    }

    let lines = split_content_lines(original);
    let line_count = lines.len();
    validate_bounds(ops, line_count)?;
    validate_no_overlap(ops)?;

    let mut replaces = Vec::new();
    let mut deletes = Vec::new();
    let mut insert_before = Vec::new();
    let mut insert_after = Vec::new();
    let mut insert_eof = Vec::new();
    for op in ops {
        match op {
            Op::Replace { .. } => replaces.push(op),
            Op::Delete { .. } => deletes.push(op),
            Op::InsertBefore { .. } => insert_before.push(op),
            Op::InsertAfter { line: Some(_), .. } => insert_after.push(op),
            Op::InsertAfter { line: None, .. } => insert_eof.push(op),
        }
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len().saturating_add(4));
    if line_count == 0 {
        for op in ops {
            match op {
                Op::InsertBefore { body, .. } | Op::InsertAfter { body, .. } => {
                    out.extend(body.iter().cloned());
                }
                Op::Replace { .. } | Op::Delete { .. } => {}
            }
        }
    } else {
        let mut index = 1usize;
        while index <= line_count {
            for op in &insert_before {
                if let Op::InsertBefore { line, body } = op {
                    if *line == index {
                        out.extend(body.iter().cloned());
                    }
                }
            }

            if let Some(Op::Replace { end, body, .. }) = replaces.iter().find_map(|op| match op {
                Op::Replace { start, .. } if *start == index => Some(*op),
                _ => None,
            }) {
                out.extend(body.iter().cloned());
                index = end + 1;
                continue;
            }

            if let Some(Op::Delete { end, .. }) = deletes.iter().find_map(|op| match op {
                Op::Delete { start, .. } if *start == index => Some(*op),
                _ => None,
            }) {
                index = end + 1;
                continue;
            }

            out.push(lines[index - 1].to_string());
            for op in &insert_after {
                if let Op::InsertAfter {
                    line: Some(line),
                    body,
                } = op
                {
                    if *line == index {
                        out.extend(body.iter().cloned());
                    }
                }
            }
            index += 1;
        }

        for op in insert_eof {
            if let Op::InsertAfter { body, .. } = op {
                out.extend(body.iter().cloned());
            }
        }
    }

    let text = finalize_text(&out, original);
    let new_tag = compute_file_hash(&text);
    Ok(ApplyOutcome {
        text,
        old_tag,
        new_tag,
    })
}

fn finalize_text(out_lines: &[String], original: &str) -> String {
    if out_lines.is_empty() {
        return String::new();
    }
    let eol = detect_eol(original);
    let mut text = out_lines.join(eol);
    if original.is_empty() || has_trailing_newline(original) {
        text.push_str(eol);
    }
    text
}

fn validate_bounds(ops: &[Op], line_count: usize) -> Result<(), String> {
    for op in ops {
        match op {
            Op::Replace { start, end, .. } | Op::Delete { start, end } => {
                if line_count == 0 {
                    return Err("cannot replace or delete lines in an empty file".into());
                }
                if *start > line_count || *end > line_count {
                    return Err(format!(
                        "line range {start}.={end} is outside the file ({line_count} line(s))"
                    ));
                }
            }
            Op::InsertBefore { line, .. } => {
                if line_count == 0 {
                    if *line != 1 {
                        return Err("insert before in an empty file must use PUT <1:".into());
                    }
                } else if *line > line_count {
                    return Err(format!(
                        "insert before line {line} is outside the file ({line_count} line(s))"
                    ));
                }
            }
            Op::InsertAfter {
                line: Some(line), ..
            } => {
                if line_count == 0 {
                    return Err(
                        "insert after a line requires a non-empty file; use PUT <1: or PUT >$:"
                            .into(),
                    );
                }
                if *line > line_count {
                    return Err(format!(
                        "insert after line {line} is outside the file ({line_count} line(s))"
                    ));
                }
            }
            Op::InsertAfter { line: None, .. } => {}
        }
    }
    Ok(())
}

fn validate_no_overlap(ops: &[Op]) -> Result<(), String> {
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    for op in ops {
        let kind = match op {
            Op::Replace { .. } => "replace",
            Op::Delete { .. } => "delete",
            Op::InsertBefore { .. } | Op::InsertAfter { .. } => continue,
        };
        if let Some((start, end)) = op.span() {
            spans.push((start, end, kind));
        }
    }
    spans.sort_by_key(|(start, _, _)| *start);
    for window in spans.windows(2) {
        let (a_start, a_end, a_kind) = window[0];
        let (b_start, b_end, b_kind) = window[1];
        if b_start <= a_end {
            return Err(format!(
                "overlapping hashline ops: {a_kind} {a_start}.={a_end} and {b_kind} {b_start}.={b_end}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
