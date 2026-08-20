use crate::model::ModelUsage;

/// Local twin of [`rho_sdk::model::InclusivePromptUsage`].
///
/// rho-providers must still compile against the published SDK, which does not
/// export that constructor yet. Keep this aligned with
/// [`rho_sdk::model::ModelUsage::from_inclusive_prompt`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InclusivePromptUsage {
    pub prompt_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reported_total: Option<u64>,
    pub context_window: Option<u64>,
    pub cost_usd_micros: Option<u64>,
}

pub(crate) fn model_usage_from_inclusive_prompt(usage: InclusivePromptUsage) -> ModelUsage {
    let input_tokens = match (
        usage.prompt_tokens,
        usage.cache_read_tokens,
        usage.cache_write_tokens,
    ) {
        (Some(prompt), cache_read, cache_write)
            if cache_read.is_some() || cache_write.is_some() =>
        {
            Some(
                prompt
                    .saturating_sub(cache_read.unwrap_or_default())
                    .saturating_sub(cache_write.unwrap_or_default()),
            )
        }
        _ => None,
    };
    let total_tokens =
        usage
            .reported_total
            .or_else(|| match (usage.prompt_tokens, usage.output_tokens) {
                (Some(prompt), Some(output)) => Some(prompt.saturating_add(output)),
                (Some(prompt), None) => Some(prompt),
                _ => None,
            });
    ModelUsage {
        input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        total_tokens,
        context_window: usage.context_window,
        cost_usd_micros: usage.cost_usd_micros,
    }
}
