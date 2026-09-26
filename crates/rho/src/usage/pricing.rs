//! Catalog-price estimates for requests whose provider did not report a cost.
//!
//! The statusline and the `/spend` report share this one formula so a live
//! session and the history agree on what a request would have cost.

use rho_providers::model::{ModelMetadata, ModelUsage};

/// Price `usage` at the catalog rates in `metadata`.
///
/// `None` means there is no catalog price for the model at this prompt size.
/// `Some(0)` is a real zero, such as a free model. A missing per-token rate
/// (commonly cache reads) prices that component at zero.
pub(crate) fn catalog_cost_usd_micros(usage: &ModelUsage, metadata: &ModelMetadata) -> Option<u64> {
    let cache_read = usage.cache_read_tokens.unwrap_or_default();
    let inclusive = usage.inclusive_prompt_tokens().unwrap_or_default();
    // Always derive billed input from inclusive prompt size. Preferring
    // `input_tokens` when Some drops mute turns after a later cache split:
    // accumulated `input_tokens` holds only the split remainder.
    let input = inclusive
        .saturating_sub(cache_read)
        .saturating_sub(usage.cache_write_tokens.unwrap_or_default());
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
    Some(micros.min(u64::MAX as u128) as u64)
}

/// Dollars for `tokens` at a per-million rate; a missing rate is free.
pub(crate) fn cost_component(tokens: u64, micros_per_million: Option<u64>) -> u128 {
    tokens as u128 * micros_per_million.unwrap_or_default() as u128 / 1_000_000
}
