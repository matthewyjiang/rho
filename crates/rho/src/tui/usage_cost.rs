use rho_providers::model::{ModelMetadata, ModelUsage};

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
    let input = usage.input_tokens.unwrap_or_default();
    let cache_read = usage.cache_read_tokens.unwrap_or_default();
    let total_input = usage.total_input_tokens().unwrap_or_default();
    let cost = metadata.cost_for_input_tokens(total_input)?;
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

fn cost_component(tokens: u64, micros_per_million: Option<u64>) -> u128 {
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
        .total_input_tokens()
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
