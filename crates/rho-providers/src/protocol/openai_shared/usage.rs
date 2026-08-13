//! Shared OpenAI usage snapshots and generation-token accounting.
//!
//! Chat Completions and Responses/Codex both report usage as JSON objects
//! with aliased field names. This module parses those snapshots, merges
//! restated running totals, and picks the output-token count that matches
//! the runtime's generation window.

use crate::{
    model::{ModelEvent, ModelUsage},
    protocol::cost::parse_usd_micros,
};

// Keep the raw 1.x carrier until rho-providers can raise its minimum rho-sdk
// version. Package verification must compile against the currently published SDK.
pub(crate) fn generation_output_tokens_event(tokens: u64) -> ModelEvent {
    ModelEvent::ProviderContext {
        kind: "rho_model_call_generation_output_tokens".into(),
        position: None,
        data: serde_json::json!({ "tokens": tokens }),
    }
}

fn generation_output_tokens_unavailable_event() -> ModelEvent {
    ModelEvent::ProviderContext {
        kind: "rho_model_call_generation_output_tokens".into(),
        position: None,
        data: serde_json::json!({ "tokens": null }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationOutputTokens {
    Unreported,
    Reported(u64),
    Unavailable,
}

impl GenerationOutputTokens {
    pub(crate) fn into_event(self) -> Option<ModelEvent> {
        match self {
            Self::Unreported => None,
            Self::Reported(tokens) => Some(generation_output_tokens_event(tokens)),
            Self::Unavailable => Some(generation_output_tokens_unavailable_event()),
        }
    }
}

/// Whether this call may have produced reasoning tokens the stream never
/// showed. Decides how to treat a usage payload that reports output tokens
/// without reasoning-token details when no reasoning deltas streamed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HiddenReasoningRisk {
    /// No serialized control asked the host to reason; treat aggregate output
    /// totals as visible-generation tokens.
    Unlikely,
    /// Reasoning was requested (or cannot be ruled out), so an aggregate total
    /// may hide off-wire reasoning whose wall time sat before the visible
    /// stream. Without reasoning-token details, report throughput as
    /// unavailable instead of an inflated rate.
    Possible,
}

/// Stream observations that decide which output-token count matches the
/// generation window measured by the runtime.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GenerationTokenContext {
    /// Whether any reasoning deltas streamed before this usage payload.
    pub(crate) reasoning_streamed: bool,
    pub(crate) hidden_reasoning_risk: HiddenReasoningRisk,
}

pub(crate) struct UsageReport {
    pub(crate) usage: ModelUsage,
    pub(crate) generation_output_tokens: GenerationOutputTokens,
}

fn extract_output_usage(usage: &serde_json::Value) -> (Option<u64>, Option<u64>) {
    for (tokens_key, details_key) in [
        ("output_tokens", "output_tokens_details"),
        ("completion_tokens", "completion_tokens_details"),
    ] {
        let Some(output_tokens) = usage.get(tokens_key).and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let reasoning_tokens = usage
            .get(details_key)
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(serde_json::Value::as_u64);
        return (Some(output_tokens), reasoning_tokens);
    }
    (None, None)
}

/// Output/reasoning token pairing from one usage payload.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReportedOutputUsage {
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_tokens: Option<u64>,
}

/// Picks the output-token count that matches the runtime's generation window.
///
/// The window opens at the first generated event, including reasoning deltas.
/// Reasoning that streamed therefore spent its wall time inside the window, so
/// the full output total is the matching numerator even when the host itemizes
/// reasoning tokens separately. Reasoning that stayed off the wire spent its
/// wall time before the window: subtract it when the host itemizes it, and
/// refuse to report a count when it might exist but cannot be separated.
pub(crate) fn resolve_generation_output_tokens(
    output_usage: Option<ReportedOutputUsage>,
    context: GenerationTokenContext,
) -> GenerationOutputTokens {
    let Some(output_usage) = output_usage else {
        return GenerationOutputTokens::Unreported;
    };
    if context.reasoning_streamed {
        return GenerationOutputTokens::Reported(output_usage.output_tokens);
    }
    match (output_usage.reasoning_tokens, context.hidden_reasoning_risk) {
        (Some(reasoning_tokens), _) => output_usage
            .output_tokens
            .checked_sub(reasoning_tokens)
            .map_or(
                GenerationOutputTokens::Unavailable,
                GenerationOutputTokens::Reported,
            ),
        (None, HiddenReasoningRisk::Unlikely) => GenerationOutputTokens::Unreported,
        (None, HiddenReasoningRisk::Possible) => GenerationOutputTokens::Unavailable,
    }
}

/// [`resolve_generation_output_tokens`] over a raw stream payload.
pub(crate) fn extract_generation_output_tokens(
    value: &serde_json::Value,
    context: GenerationTokenContext,
) -> GenerationOutputTokens {
    let Some(usage) = value.get("usage").filter(|usage| usage.is_object()) else {
        return GenerationOutputTokens::Unreported;
    };
    let (output_tokens, reasoning_tokens) = extract_output_usage(usage);
    resolve_generation_output_tokens(
        output_tokens.map(|output_tokens| ReportedOutputUsage {
            output_tokens,
            reasoning_tokens,
        }),
        context,
    )
}

pub(crate) fn extract_usage_report(
    value: &serde_json::Value,
    context: GenerationTokenContext,
) -> Option<UsageReport> {
    Some(UsageReport {
        usage: extract_usage(value)?,
        generation_output_tokens: extract_generation_output_tokens(value, context),
    })
}

/// Usage fields as the host reported them, before cache-bucket derivation.
///
/// Snapshots merge at this raw level: deriving `ModelUsage` per snapshot
/// would let a later snapshot's cache-adjusted input combine with cache
/// buckets retained from an earlier one, double-counting cached tokens.
///
/// Output and reasoning tokens stay paired: a later snapshot that reports an
/// output count replaces both, so a partial restatement cannot keep an older
/// reasoning count next to a newer output total.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RawUsage {
    /// Raw input total; cache reads and writes are subsets of this count.
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    total_tokens: Option<u64>,
    context_window: Option<u64>,
    cost_usd_micros: Option<u64>,
}

impl RawUsage {
    /// Field-wise cumulative merge: a later snapshot wins where it reports a
    /// field, earlier totals survive where it does not. Output and reasoning
    /// stay atomic.
    pub(crate) fn merge(self, observed: Self) -> Self {
        let (output_tokens, reasoning_tokens) = if observed.output_tokens.is_some() {
            (observed.output_tokens, observed.reasoning_tokens)
        } else {
            (self.output_tokens, self.reasoning_tokens)
        };
        Self {
            input_tokens: observed.input_tokens.or(self.input_tokens),
            output_tokens,
            reasoning_tokens,
            cache_read_tokens: observed.cache_read_tokens.or(self.cache_read_tokens),
            cache_write_tokens: observed.cache_write_tokens.or(self.cache_write_tokens),
            total_tokens: observed.total_tokens.or(self.total_tokens),
            context_window: observed.context_window.or(self.context_window),
            cost_usd_micros: observed.cost_usd_micros.or(self.cost_usd_micros),
        }
    }

    /// OpenAI reports cache reads and writes as subsets of the raw input
    /// count, while `ModelUsage` keeps the three input buckets disjoint.
    pub(crate) fn into_model_usage(self) -> ModelUsage {
        let input_tokens = self.input_tokens.map(|input| {
            input
                .saturating_sub(self.cache_read_tokens.unwrap_or_default())
                .saturating_sub(self.cache_write_tokens.unwrap_or_default())
        });
        ModelUsage {
            input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            total_tokens: self.total_tokens,
            context_window: self.context_window,
            cost_usd_micros: self.cost_usd_micros,
        }
    }

    pub(crate) fn reported_output(self) -> Option<ReportedOutputUsage> {
        self.output_tokens.map(|output_tokens| ReportedOutputUsage {
            output_tokens,
            reasoning_tokens: self.reasoning_tokens,
        })
    }
}

pub(crate) fn extract_usage(value: &serde_json::Value) -> Option<ModelUsage> {
    extract_raw_usage(value).map(RawUsage::into_model_usage)
}

pub(crate) fn extract_raw_usage(value: &serde_json::Value) -> Option<RawUsage> {
    let usage = value.get("usage").filter(|usage| usage.is_object())?;
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(|v| v.as_u64());
    let (output_tokens, reasoning_tokens) = extract_output_usage(usage);
    let total_tokens = usage.get("total_tokens").and_then(|v| v.as_u64());
    let input_details = usage
        .get("input_tokens_details")
        .or_else(|| usage.get("prompt_tokens_details"));
    let cache_read_tokens = input_details
        .and_then(|v| {
            v.get("cached_tokens")
                .or_else(|| v.get("cache_read_tokens"))
                .or_else(|| v.get("cached_input_tokens"))
        })
        .and_then(|v| v.as_u64());
    let cache_write_tokens = input_details
        .and_then(|v| {
            v.get("cache_write_tokens")
                .or_else(|| v.get("cache_creation_input_tokens"))
                .or_else(|| v.get("cache_creation_tokens"))
        })
        .and_then(|v| v.as_u64());
    let context_window = usage
        .get("context_window")
        .or_else(|| usage.get("context_window_tokens"))
        .and_then(|v| v.as_u64());
    let reported_cost = [
        usage.get("cost_usd"),
        usage.get("estimated_cost_usd"),
        usage.get("cost"),
        usage.get("estimated_cost"),
    ]
    .into_iter()
    .flatten()
    .find_map(parse_usd_micros);
    let upstream_cost = usage
        .get("cost_details")
        .and_then(|details| details.get("upstream_inference_cost"))
        .and_then(parse_usd_micros);
    let cost_usd_micros = match (reported_cost, upstream_cost) {
        (Some(reported), Some(upstream)) => Some(reported.saturating_add(upstream)),
        (Some(reported), None) => Some(reported),
        (None, Some(upstream)) => Some(upstream),
        (None, None) => None,
    };

    Some(RawUsage {
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens,
        context_window,
        cost_usd_micros,
    })
}

#[cfg(test)]
#[path = "stream_cost_tests.rs"]
mod stream_cost_tests;
