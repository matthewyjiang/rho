//! A session's running cost for hosts that display one, such as ACP's
//! `usage_update`.
//!
//! The total comes from the durable ledger, not from in-process counters, so
//! it survives a restart and `session/load`, and every provider request
//! (retried attempts, compaction, titles, subagents) counts exactly once.

use super::report::{catalog_route_pricing, load_session_spend, Valuation};

/// Cumulative spend of `session_id`, priced like `/spend`.
///
/// An unreadable ledger counts as [`Valuation::Unpriced`]: a cost display
/// can wait for the next read. Unit tests never record to the default
/// ledger, so they always get `Unpriced`.
pub(crate) async fn session_valuation(session_id: &rho_sdk::SessionId) -> Valuation {
    if cfg!(test) {
        return Valuation::Unpriced;
    }
    let Ok(path) = crate::paths::usage_database_path() else {
        return Valuation::Unpriced;
    };
    let session_id = session_id.to_string();
    let spend = tokio::task::spawn_blocking(move || {
        load_session_spend(&path, &session_id, catalog_route_pricing)
    })
    .await;
    match spend {
        Ok(Ok(totals)) => totals.valuation(),
        Ok(Err(error)) => {
            tracing::debug!(%error, "could not read session cost from the usage ledger");
            Valuation::Unpriced
        }
        Err(error) => {
            tracing::debug!(%error, "session cost task failed");
            Valuation::Unpriced
        }
    }
}
