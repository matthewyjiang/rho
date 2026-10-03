//! Keeps the classifier transcript inside the classifier model's context.
//!
//! Two mechanisms, applied in order:
//! 1. Completed tool calls have their string arguments capped at
//!    [`COMPLETED_CALL_STRING_CAP_CHARS`]. Their bodies (file contents, patches)
//!    are rarely authorization evidence.
//! 2. When a [`TranscriptBudget`] is set and the transcript still exceeds it,
//!    the oldest completed tool calls are dropped and replaced by one
//!    `omitted_tool_calls` record. User text, questionnaire answers, in-flight
//!    tool calls, and the pending capability are never dropped; if they alone
//!    exceed the budget, fitting fails with [`TranscriptOverBudget`].

use rho_sdk::model::context::estimate_text_tokens;
use serde_json::Value;

/// Per-string cap for completed tool-call arguments.
///
/// Measured on 245 local sessions: 500 chars keeps 82% of historical `bash`
/// commands intact (p50 214 chars) while cutting the estimated p99 transcript
/// from ~63K to ~32K tokens.
pub(super) const COMPLETED_CALL_STRING_CAP_CHARS: usize = 500;

/// Token limit the rendered transcript must fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptBudget {
    /// The classifier model's context window is unknown; only the string cap applies.
    Unbounded,
    /// Estimated transcript tokens must not exceed this value.
    Tokens(u64),
}

/// The transcript cannot fit the budget even after dropping every droppable record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("classifier context over budget: needs ~{estimated_tokens} tokens, limit {limit_tokens}")]
pub(crate) struct TranscriptOverBudget {
    pub estimated_tokens: u64,
    pub limit_tokens: u64,
}

/// Whether budget fitting may drop a history record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Retention {
    Required,
    Droppable,
}

/// One rendered transcript line before budget fitting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TranscriptLine {
    pub text: String,
    pub retention: Retention,
}

/// Returns `value` with every string longer than `cap_chars` cut to that many
/// chars plus a `…[+N chars]` marker. Structure and non-string values stay.
pub(super) fn cap_string_leaves(value: &Value, cap_chars: usize) -> Value {
    match value {
        Value::String(text) => {
            let total = text.chars().count();
            if total <= cap_chars {
                return value.clone();
            }
            let kept: String = text.chars().take(cap_chars).collect();
            Value::String(format!("{kept}…[+{} chars]", total - cap_chars))
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| cap_string_leaves(item, cap_chars))
                .collect(),
        ),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, item)| (name.clone(), cap_string_leaves(item, cap_chars)))
                .collect(),
        ),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

/// Joins `history` and `tail` into the final transcript, dropping the oldest
/// droppable history lines until the estimate fits `budget`.
///
/// The estimate rounds each line up separately, so it slightly overcounts
/// the joined text.
pub(super) fn fit_transcript(
    history: Vec<TranscriptLine>,
    tail: Vec<String>,
    budget: TranscriptBudget,
) -> Result<String, TranscriptOverBudget> {
    let line_tokens = |text: &str| estimate_text_tokens(text).saturating_add(1);
    let mut total: u64 = history
        .iter()
        .map(|line| line_tokens(&line.text))
        .chain(tail.iter().map(|line| line_tokens(line)))
        .sum();
    let limit = match budget {
        TranscriptBudget::Tokens(limit) if total > limit => limit,
        TranscriptBudget::Unbounded | TranscriptBudget::Tokens(_) => {
            return Ok(join(history.into_iter().map(|line| line.text), tail));
        }
    };

    let mut keep = vec![true; history.len()];
    let mut omitted = 0_usize;
    let mut marker_tokens = 0;
    for (index, line) in history.iter().enumerate() {
        if total.saturating_add(marker_tokens) <= limit {
            break;
        }
        if line.retention == Retention::Droppable {
            keep[index] = false;
            total -= line_tokens(&line.text);
            omitted += 1;
            marker_tokens = line_tokens(&omitted_marker(omitted));
        }
    }
    let estimated_tokens = total.saturating_add(marker_tokens);
    if estimated_tokens > limit {
        return Err(TranscriptOverBudget {
            estimated_tokens,
            limit_tokens: limit,
        });
    }

    let kept = history
        .into_iter()
        .zip(keep)
        .filter_map(|(line, keep)| keep.then_some(line.text));
    Ok(join(
        std::iter::once(omitted_marker(omitted)).chain(kept),
        tail,
    ))
}

fn omitted_marker(count: usize) -> String {
    format!("\"omitted_tool_calls\" count={count}")
}

fn join(history: impl Iterator<Item = String>, tail: Vec<String>) -> String {
    history.chain(tail).collect::<Vec<_>>().join("\n")
}
