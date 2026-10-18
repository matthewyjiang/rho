//! A session's running cost for hosts that display one, such as ACP's
//! `usage_update`.
//!
//! The total comes from the durable ledger, not from in-process counters, so
//! it survives a restart and `session/load`, and every provider request
//! (retried attempts, compaction, titles, subagents) counts exactly once.

use super::report::{catalog_route_pricing, load_session_spend, Valuation};

/// Cumulative cost of `session_id` in USD micros, priced like `/spend`.
///
/// `None` when nothing so far has a price: no requests yet, only unpriced or
/// local models, or an unreadable ledger. Unit tests never record to the
/// default ledger, so they always get `None`.
pub(crate) async fn session_cost_usd_micros(session_id: &rho_sdk::SessionId) -> Option<u64> {
    if cfg!(test) {
        return None;
    }
    let path = crate::paths::usage_database_path().ok()?;
    let session_id = session_id.to_string();
    let spend = tokio::task::spawn_blocking(move || {
        load_session_spend(&path, &session_id, catalog_route_pricing)
    })
    .await;
    match spend {
        Ok(Ok(totals)) => match totals.valuation() {
            Valuation::Usd(micros) => Some(micros),
            Valuation::Local | Valuation::Unpriced => None,
        },
        Ok(Err(error)) => {
            tracing::debug!(%error, "could not read session cost from the usage ledger");
            None
        }
        Err(error) => {
            tracing::debug!(%error, "session cost task failed");
            None
        }
    }
}
