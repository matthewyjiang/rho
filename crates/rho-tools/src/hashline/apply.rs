//! Pure apply of parsed hashline ops onto a text body.

use std::collections::BTreeMap;
use std::fmt;

use super::{
    format::{compute_file_hash, detect_eol, has_trailing_newline, split_content_lines},
    parser::Op,
};

/// Result of applying ops to one file body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ApplyOutcome {
    pub text: String,
    /// 1-indexed post-edit lines touched by the ops (inserts, replacements, and
    /// neighbors of pure deletes). Preview rendering uses this set instead of
    /// recomputing a content diff.
    pub focus_lines: Vec<usize>,
}

/// Typed apply failure. Callers match variants instead of English substrings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ApplyError {
    TagMismatch { expected: String, live: String },
    EmptyOps,
    Message(String),
}

impl ApplyError {
    fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TagMismatch { expected, live } => write!(
                f,
                "hashline tag mismatch: expected {expected}, live file is {live}. Re-read the path or copy the latest edit preview header."
            ),
            Self::EmptyOps => write!(f, "hashline section has no operations"),
            Self::Message(message) => write!(f, "{message}"),
        }
    }
}

/// Apply `ops` to `original` after verifying `expected_tag`.
///
/// All ops address **original** line numbers from the tagged snapshot. Every op
/// is planned against the line it anchors to, so destructive spans that overlap
/// each other, or that swallow another op's anchor, fail closed instead of
/// silently dropping work.
///
/// Takes ownership of `ops` so body lines move into the plan instead of cloning.
pub(crate) fn apply_ops(
    original: &str,
    expected_tag: &str,
    ops: Vec<Op>,
) -> Result<ApplyOutcome, ApplyError> {
    let live_tag = compute_file_hash(original);
    if !live_tag.eq_ignore_ascii_case(expected_tag) {
        return Err(ApplyError::TagMismatch {
            expected: expected_tag.to_string(),
            live: live_tag,
        });
    }
    if ops.is_empty() {
        return Err(ApplyError::EmptyOps);
    }

    let lines = split_content_lines(original);
    let plan = Plan::build(ops, lines.len())?;
    let emitted = plan.emit(&lines);
    let text = finalize_text(&emitted.lines, original);
    Ok(ApplyOutcome {
        text,
        focus_lines: emitted.focus_lines,
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

struct EmitResult {
    lines: Vec<String>,
    focus_lines: Vec<usize>,
}

impl Plan {
    /// Bucket every op onto its anchor line, rejecting out-of-range anchors and
    /// destructive spans that collide with another op.
    fn build(ops: Vec<Op>, line_count: usize) -> Result<Self, ApplyError> {
        let mut plan = Self {
            slots: BTreeMap::new(),
            eof: Vec::new(),
        };
        for op in ops {
            match op {
                Op::Replace { start, end, body } => {
                    plan.add_span(start, end, body, "replace", line_count)?;
                }
                Op::Delete { start, end } => {
                    plan.add_span(start, end, Vec::new(), "delete", line_count)?;
                }
                Op::InsertBefore { line, body } => {
                    // An empty file has no line 1 to sit before, so head inserts
                    // and end-of-file appends are the same position.
                    if line_count == 0 {
                        if line != 1 {
                            return Err(ApplyError::message(
                                "insert before in an empty file must use PUT <1:",
                            ));
                        }
                        plan.eof.extend(body);
                        continue;
                    }
                    Self::check_in_range(line, line_count, "insert before")?;
                    plan.slot(line).before.extend(body);
                }
                Op::InsertAfter {
                    line: Some(line),
                    body,
                } => {
                    if line_count == 0 {
                        return Err(ApplyError::message(
                            "insert after a line requires a non-empty file; use PUT <1: or PUT >$:",
                        ));
                    }
                    Self::check_in_range(line, line_count, "insert after")?;
                    plan.slot(line).after.extend(body);
                }
                Op::InsertAfter { line: None, body } => plan.eof.extend(body),
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
    ) -> Result<(), ApplyError> {
        if line_count == 0 {
            return Err(ApplyError::message(
                "cannot replace or delete lines in an empty file",
            ));
        }
        if start > line_count || end > line_count {
            return Err(ApplyError::message(format!(
                "line range {start}.={end} is outside the file ({line_count} line(s))"
            )));
        }
        let slot = self.slots.entry(start).or_default();
        if let Some(existing) = &slot.span {
            return Err(ApplyError::message(format!(
                "overlapping hashline ops: {} {start}.={} and {kind} {start}.={end}",
                existing.kind, existing.end
            )));
        }
        slot.span = Some(SpanEdit { end, body, kind });
        Ok(())
    }

    /// Reject destructive spans that overlap each other or swallow another op's
    /// anchor. Without this, an insert inside a replaced range would be planned
    /// but never emitted.
    fn validate_spans(&self) -> Result<(), ApplyError> {
        let mut active: Option<(usize, &SpanEdit)> = None;
        for (line, slot) in &self.slots {
            if let Some((start, span)) = active {
                if *line <= span.end {
                    return Err(ApplyError::message(match &slot.span {
                        Some(other) => format!(
                            "overlapping hashline ops: {} {start}.={} and {} {line}.={}",
                            span.kind, span.end, other.kind, other.end
                        ),
                        None => format!(
                            "hashline op anchored at line {line} falls inside {} {start}.={}; anchor it outside the range",
                            span.kind, span.end
                        ),
                    }));
                }
                active = None;
            }
            if let Some(span) = &slot.span {
                // `after` rows on the first line of a multi-line span would land
                // in the middle of the range the span removes.
                if span.end > *line && !slot.after.is_empty() {
                    return Err(ApplyError::message(format!(
                        "insert after line {line} falls inside {} {line}.={}; anchor it outside the range",
                        span.kind, span.end
                    )));
                }
                active = Some((*line, span));
            }
        }
        Ok(())
    }

    fn check_in_range(line: usize, line_count: usize, label: &str) -> Result<(), ApplyError> {
        if line > line_count {
            return Err(ApplyError::message(format!(
                "{label} line {line} is outside the file ({line_count} line(s))"
            )));
        }
        Ok(())
    }

    /// Walk the original lines once, splicing in each anchor's edits and
    /// recording post-edit focus lines for the chain preview.
    fn emit(&self, lines: &[&str]) -> EmitResult {
        let mut out = Vec::with_capacity(lines.len() + self.eof.len());
        let mut focus = Vec::new();
        let mut index = 1;
        while index <= lines.len() {
            let Some(slot) = self.slots.get(&index) else {
                out.push(lines[index - 1].to_string());
                index += 1;
                continue;
            };

            push_body(&mut out, &mut focus, &slot.before);

            match &slot.span {
                Some(span) if span.body.is_empty() => {
                    // Pure delete: keep neighbors so the gap stays editable.
                    if !out.is_empty() {
                        focus.push(out.len());
                    }
                    let next_neighbor = out.len() + slot.after.len() + 1;
                    index = span.end + 1;
                    if index <= lines.len() {
                        focus.push(next_neighbor);
                    }
                }
                Some(span) => {
                    push_body(&mut out, &mut focus, &span.body);
                    index = span.end + 1;
                }
                None => {
                    out.push(lines[index - 1].to_string());
                    index += 1;
                }
            }

            push_body(&mut out, &mut focus, &slot.after);
        }

        push_body(&mut out, &mut focus, &self.eof);

        // Drop focus markers past EOF (trailing delete with no next neighbor).
        focus.retain(|line| *line >= 1 && *line <= out.len());
        focus.sort_unstable();
        focus.dedup();
        if focus.is_empty() && !out.is_empty() {
            focus.push(1);
        }

        EmitResult {
            lines: out,
            focus_lines: focus,
        }
    }
}

fn push_body(out: &mut Vec<String>, focus: &mut Vec<usize>, body: &[String]) {
    for line in body {
        out.push(line.clone());
        focus.push(out.len());
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
