//! Pure apply of parsed hashline ops onto a text body.

use std::collections::BTreeMap;

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
/// All ops address **original** line numbers from the tagged snapshot. Every op
/// is planned against the line it anchors to, so destructive spans that overlap
/// each other, or that swallow another op's anchor, fail closed instead of
/// silently dropping work.
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
    let plan = Plan::build(ops, lines.len())?;
    let text = finalize_text(&plan.emit(&lines), original);
    let new_tag = compute_file_hash(&text);
    Ok(ApplyOutcome {
        text,
        old_tag,
        new_tag,
    })
}

/// A destructive edit that consumes original lines `[start, end]`.
struct SpanEdit {
    end: usize,
    /// Replacement rows; empty for a delete.
    body: Vec<String>,
    kind: &'static str,
}

/// Every edit anchored to one original line, in document order.
#[derive(Default)]
struct LineSlot {
    before: Vec<String>,
    span: Option<SpanEdit>,
    after: Vec<String>,
}

/// All ops for one file, keyed by the original line they anchor to.
struct Plan {
    slots: BTreeMap<usize, LineSlot>,
    eof: Vec<String>,
}

impl Plan {
    /// Bucket every op onto its anchor line, rejecting out-of-range anchors and
    /// destructive spans that collide with another op.
    fn build(ops: &[Op], line_count: usize) -> Result<Self, String> {
        let mut plan = Self {
            slots: BTreeMap::new(),
            eof: Vec::new(),
        };
        for op in ops {
            match op {
                Op::Replace { start, end, body } => {
                    plan.add_span(*start, *end, body.clone(), "replace", line_count)?;
                }
                Op::Delete { start, end } => {
                    plan.add_span(*start, *end, Vec::new(), "delete", line_count)?;
                }
                Op::InsertBefore { line, body } => {
                    // An empty file has no line 1 to sit before, so head inserts
                    // and end-of-file appends are the same position.
                    if line_count == 0 {
                        if *line != 1 {
                            return Err("insert before in an empty file must use PUT <1:".into());
                        }
                        plan.eof.extend(body.iter().cloned());
                        continue;
                    }
                    Self::check_in_range(*line, line_count, "insert before")?;
                    plan.slot(*line).before.extend(body.iter().cloned());
                }
                Op::InsertAfter {
                    line: Some(line),
                    body,
                } => {
                    if line_count == 0 {
                        return Err(
                            "insert after a line requires a non-empty file; use PUT <1: or PUT >$:"
                                .into(),
                        );
                    }
                    Self::check_in_range(*line, line_count, "insert after")?;
                    plan.slot(*line).after.extend(body.iter().cloned());
                }
                Op::InsertAfter { line: None, body } => plan.eof.extend(body.iter().cloned()),
            }
        }
        plan.validate_spans()?;
        Ok(plan)
    }

    fn slot(&mut self, line: usize) -> &mut LineSlot {
        self.slots.entry(line).or_default()
    }

    fn add_span(
        &mut self,
        start: usize,
        end: usize,
        body: Vec<String>,
        kind: &'static str,
        line_count: usize,
    ) -> Result<(), String> {
        if line_count == 0 {
            return Err("cannot replace or delete lines in an empty file".into());
        }
        if start > line_count || end > line_count {
            return Err(format!(
                "line range {start}.={end} is outside the file ({line_count} line(s))"
            ));
        }
        let slot = self.slots.entry(start).or_default();
        if let Some(existing) = &slot.span {
            return Err(format!(
                "overlapping hashline ops: {} {start}.={} and {kind} {start}.={end}",
                existing.kind, existing.end
            ));
        }
        slot.span = Some(SpanEdit { end, body, kind });
        Ok(())
    }

    /// Reject destructive spans that overlap each other or swallow another op's
    /// anchor. Without this, an insert inside a replaced range would be planned
    /// but never emitted.
    fn validate_spans(&self) -> Result<(), String> {
        let mut active: Option<(usize, &SpanEdit)> = None;
        for (line, slot) in &self.slots {
            if let Some((start, span)) = active {
                if *line <= span.end {
                    return Err(match &slot.span {
                        Some(other) => format!(
                            "overlapping hashline ops: {} {start}.={} and {} {line}.={}",
                            span.kind, span.end, other.kind, other.end
                        ),
                        None => format!(
                            "hashline op anchored at line {line} falls inside {} {start}.={}; anchor it outside the range",
                            span.kind, span.end
                        ),
                    });
                }
                active = None;
            }
            if let Some(span) = &slot.span {
                // `after` rows on the first line of a multi-line span would land
                // in the middle of the range the span removes.
                if span.end > *line && !slot.after.is_empty() {
                    return Err(format!(
                        "insert after line {line} falls inside {} {line}.={}; anchor it outside the range",
                        span.kind, span.end
                    ));
                }
                active = Some((*line, span));
            }
        }
        Ok(())
    }

    fn check_in_range(line: usize, line_count: usize, label: &str) -> Result<(), String> {
        if line > line_count {
            return Err(format!(
                "{label} line {line} is outside the file ({line_count} line(s))"
            ));
        }
        Ok(())
    }

    /// Walk the original lines once, splicing in each anchor's edits.
    fn emit(&self, lines: &[&str]) -> Vec<String> {
        let mut out = Vec::with_capacity(lines.len() + self.eof.len());
        let mut index = 1;
        while index <= lines.len() {
            let Some(slot) = self.slots.get(&index) else {
                out.push(lines[index - 1].to_string());
                index += 1;
                continue;
            };
            out.extend(slot.before.iter().cloned());
            match &slot.span {
                Some(span) => {
                    out.extend(span.body.iter().cloned());
                    index = span.end + 1;
                }
                None => {
                    out.push(lines[index - 1].to_string());
                    index += 1;
                }
            }
            out.extend(slot.after.iter().cloned());
        }
        out.extend(self.eof.iter().cloned());
        out
    }
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

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
