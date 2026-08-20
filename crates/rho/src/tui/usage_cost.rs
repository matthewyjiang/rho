use rho_providers::model::{ModelMetadata, ModelUsage};
use rho_sdk::model::context::estimate_text_tokens;

/// Attempt-aware provider usage snapshots for a single run.
///
/// Provider usage is cumulative within the current attempt. Failed attempts keep
/// their already-charged tokens via [`Self::before_attempt`], while step
/// boundaries are tracked so callers can derive last-step deltas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AttemptAwareRunUsage {
    before_step: Option<ModelUsage>,
    before_attempt: Option<ModelUsage>,
    current: Option<ModelUsage>,
}

impl AttemptAwareRunUsage {
    pub(super) fn current(&self) -> Option<&ModelUsage> {
        self.current.as_ref()
    }

    pub(super) fn current_mut(&mut self) -> Option<&mut ModelUsage> {
        self.current.as_mut()
    }

    pub(super) fn before_step(&self) -> Option<&ModelUsage> {
        self.before_step.as_ref()
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn step_started(&mut self) {
        self.before_step = self.current.clone();
        self.before_attempt = None;
    }

    pub(super) fn attempt_reset(&mut self) {
        self.before_attempt = self
            .current
            .as_ref()
            .map(|usage| usage_difference(usage, self.before_step.as_ref()));
    }

    /// Apply a provider usage snapshot for the active attempt.
    ///
    /// `prepare_retry_snapshot` runs only when merging onto failed-attempt
    /// tokens (main TUI uses it to estimate missing costs before merge).
    pub(super) fn apply_snapshot(
        &mut self,
        usage: ModelUsage,
        prepare_retry_snapshot: impl FnOnce(ModelUsage) -> ModelUsage,
    ) -> ModelUsage {
        let mut current_run_usage = usage;
        if let Some(attempt_baseline) = &self.before_attempt {
            current_run_usage = prepare_retry_snapshot(current_run_usage);
            let mut combined = None;
            merge_usage(&mut combined, attempt_baseline.clone());
            merge_usage(&mut combined, current_run_usage);
            current_run_usage = combined.expect("attempt baseline is present");
        }
        self.current = Some(current_run_usage.clone());
        current_run_usage
    }
}

/// Display-only generation for the in-flight provider stream.
///
/// Quiet hosts (OpenAI-compatible chat) often withhold usage until the final
/// chunk. This estimate meters streamed output so statusline cost can advance
/// during the attempt, then yields as soon as any provider `Usage` arrives.
/// It must never enter the durable usage ledger, and it must not restate the
/// prompt as new uncached input: `ContextEstimated` is the full window, so
/// billing it on submit double-counts history and ignores cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LiveStreamUsageEstimate {
    output_tokens: u64,
    provider_usage_seen: bool,
}

impl LiveStreamUsageEstimate {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn add_output_text(&mut self, text: &str) {
        self.add_output_tokens(estimate_text_tokens(text));
    }

    pub(super) fn add_output_tokens(&mut self, tokens: u64) {
        if self.provider_usage_seen || tokens == 0 {
            return;
        }
        self.output_tokens = self.output_tokens.saturating_add(tokens);
    }

    pub(super) fn provider_usage_received(&mut self) {
        self.provider_usage_seen = true;
        self.output_tokens = 0;
    }

    pub(super) fn is_active(&self) -> bool {
        !self.provider_usage_seen && self.output_tokens > 0
    }

    pub(super) fn as_usage(&self) -> Option<ModelUsage> {
        if !self.is_active() {
            return None;
        }
        Some(ModelUsage {
            output_tokens: Some(self.output_tokens),
            ..ModelUsage::default()
        })
    }
}

/// Merge durable cumulative usage with an active live stream estimate for display.
pub(super) fn display_usage_with_live(
    cumulative: Option<&ModelUsage>,
    live: &LiveStreamUsageEstimate,
    metadata: Option<&ModelMetadata>,
) -> Option<ModelUsage> {
    let live_usage = live
        .as_usage()
        .map(|usage| usage_with_estimated_cost(usage, metadata));
    match (cumulative.cloned(), live_usage) {
        (None, None) => None,
        (Some(cumulative), None) => Some(cumulative),
        (None, Some(live_usage)) => Some(live_usage),
        (Some(cumulative), Some(live_usage)) => {
            let mut combined = Some(cumulative);
            merge_usage(&mut combined, live_usage);
            combined
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum CostSource {
    #[default]
    ProviderReported,
    Estimated,
}

impl CostSource {
    fn combine(self, other: Self) -> Self {
        if self == Self::Estimated || other == Self::Estimated {
            Self::Estimated
        } else {
            Self::ProviderReported
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct UsageCostTracker {
    cumulative: CostSource,
    before_run: CostSource,
    failed_attempts: CostSource,
    current_snapshot: CostSource,
}

impl UsageCostTracker {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn run_started(&mut self) {
        self.before_run = self.cumulative;
        self.failed_attempts = CostSource::ProviderReported;
        self.current_snapshot = CostSource::ProviderReported;
    }

    pub(super) fn step_started(&mut self) {
        self.current_snapshot = CostSource::ProviderReported;
    }

    pub(super) fn attempt_restarted(&mut self) {
        self.failed_attempts = self.failed_attempts.combine(self.current_snapshot);
        self.current_snapshot = CostSource::ProviderReported;
    }

    pub(super) fn record_usage(&mut self, usage: &ModelUsage) -> CostSource {
        let latest = if usage.cost_usd_micros.is_some() {
            CostSource::ProviderReported
        } else {
            CostSource::Estimated
        };
        self.current_snapshot = latest;
        let current_run = self.failed_attempts.combine(self.current_snapshot);
        self.cumulative = self.before_run.combine(current_run);
        current_run
    }

    pub(super) fn cumulative_source(self) -> CostSource {
        self.cumulative
    }
}

pub(super) fn estimated_cost_usd_micros(
    usage: &ModelUsage,
    metadata: Option<&ModelMetadata>,
) -> Option<u64> {
    let metadata = metadata?;
    let cache_read = usage.cache_read_tokens.unwrap_or_default();
    let inclusive = usage.inclusive_prompt_tokens().unwrap_or_default();
    let input = match usage.input_tokens {
        Some(input) => input,
        None => inclusive
            .saturating_sub(cache_read)
            .saturating_sub(usage.cache_write_tokens.unwrap_or_default()),
    };
    let cost = metadata.cost_for_input_tokens(inclusive)?;
    let mut micros = 0u128;
    micros += cost_component(input, cost.input_micros_per_m);
    micros += cost_component(
        usage.output_tokens.unwrap_or_default(),
        cost.output_micros_per_m,
    );
    micros += cost_component(cache_read, cost.cache_read_micros_per_m);
    micros += cost_component(
        usage.cache_write_tokens.unwrap_or_default(),
        cost.cache_write_micros_per_m,
    );
    (micros > 0).then_some(micros.min(u64::MAX as u128) as u64)
}

pub(super) fn format_usd(micros: u64) -> String {
    let dollars = micros as f64 / 1_000_000.0;
    if dollars >= 100.0 {
        format!("${dollars:.0}")
    } else if dollars >= 10.0 {
        format!("${dollars:.2}")
    } else {
        format!("${dollars:.3}")
    }
}

pub(super) fn format_token_count(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    }
}

/// Compact in/out/cache breakdown for status and attach headers.
pub(super) fn format_usage_token_summary(usage: &ModelUsage) -> Option<String> {
    let mut parts = Vec::new();
    push_token_part(&mut parts, "in", display_input_tokens(usage));
    push_token_part(&mut parts, "out", usage.output_tokens);
    push_token_part(&mut parts, "cache r", usage.cache_read_tokens);
    push_token_part(&mut parts, "cache w", usage.cache_write_tokens);
    (!parts.is_empty()).then(|| format!("tokens {}", parts.join(" · ")))
}

pub(super) fn display_input_tokens(usage: &ModelUsage) -> Option<u64> {
    usage.input_tokens.or_else(|| {
        let has_cache_split =
            usage.cache_read_tokens.is_some() || usage.cache_write_tokens.is_some();
        (!has_cache_split)
            .then(|| usage.inclusive_prompt_tokens())
            .flatten()
    })
}

fn push_token_part(parts: &mut Vec<String>, label: &str, tokens: Option<u64>) {
    if let Some(tokens) = tokens {
        parts.push(format!("{label} {}", format_token_count(tokens)));
    }
}

/// Resolve provider-reported or estimated main-session cost.
pub(super) fn resolved_usage_cost_usd_micros(
    usage: &ModelUsage,
    metadata: Option<&ModelMetadata>,
) -> Option<u64> {
    usage
        .cost_usd_micros
        .or_else(|| estimated_cost_usd_micros(usage, metadata))
}

/// Combine main-session cost with already-aggregated non-main cost (subagents,
/// advisor, and any future extras folded at the call site).
pub(super) fn session_total_cost_usd_micros(
    main_cost_micros: Option<u64>,
    extra_cost_usd_micros: u64,
) -> Option<u64> {
    match (main_cost_micros, extra_cost_usd_micros) {
        (None, 0) => None,
        (main, extra) => Some(main.unwrap_or(0).saturating_add(extra)),
    }
}

pub(super) fn cost_component(tokens: u64, micros_per_million: Option<u64>) -> u128 {
    tokens as u128 * micros_per_million.unwrap_or_default() as u128 / 1_000_000
}

#[cfg(test)]
#[path = "usage_cost_tests.rs"]
mod tests;

pub(super) fn usage_with_estimated_cost(
    mut usage: ModelUsage,
    metadata: Option<&ModelMetadata>,
) -> ModelUsage {
    if usage.cost_usd_micros.is_none() {
        usage.cost_usd_micros = estimated_cost_usd_micros(&usage, metadata);
    }
    usage
}

pub(super) fn usage_difference(usage: &ModelUsage, baseline: Option<&ModelUsage>) -> ModelUsage {
    let baseline = baseline.cloned().unwrap_or_default();
    ModelUsage {
        input_tokens: subtract_optional(usage.input_tokens, baseline.input_tokens),
        output_tokens: subtract_optional(usage.output_tokens, baseline.output_tokens),
        cache_read_tokens: subtract_optional(usage.cache_read_tokens, baseline.cache_read_tokens),
        cache_write_tokens: subtract_optional(
            usage.cache_write_tokens,
            baseline.cache_write_tokens,
        ),
        total_tokens: subtract_optional(usage.total_tokens, baseline.total_tokens),
        context_window: usage.context_window,
        cost_usd_micros: subtract_optional(usage.cost_usd_micros, baseline.cost_usd_micros),
    }
}

pub(super) fn subtract_optional(value: Option<u64>, baseline: Option<u64>) -> Option<u64> {
    value.map(|value| value.saturating_sub(baseline.unwrap_or_default()))
}

pub(super) fn merge_usage(total: &mut Option<ModelUsage>, mut usage: ModelUsage) {
    usage.total_tokens = usage.total_tokens.or_else(|| usage_total_tokens(&usage));
    let Some(total) = total.as_mut() else {
        *total = Some(usage);
        return;
    };
    total.input_tokens = add_optional(total.input_tokens, usage.input_tokens);
    total.output_tokens = add_optional(total.output_tokens, usage.output_tokens);
    total.cache_read_tokens = add_optional(total.cache_read_tokens, usage.cache_read_tokens);
    total.cache_write_tokens = add_optional(total.cache_write_tokens, usage.cache_write_tokens);
    total.total_tokens = add_optional(total.total_tokens, usage.total_tokens);
    total.cost_usd_micros = add_optional(total.cost_usd_micros, usage.cost_usd_micros);
    total.context_window = usage.context_window.or(total.context_window);
}

pub(super) fn usage_total_tokens(usage: &ModelUsage) -> Option<u64> {
    let total = usage
        .inclusive_prompt_tokens()
        .unwrap_or_default()
        .saturating_add(usage.output_tokens.unwrap_or_default());
    (total > 0).then_some(total)
}

pub(super) fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}
