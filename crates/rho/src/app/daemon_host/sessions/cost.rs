//! The session's cost on its usage events (#1670), read from the durable
//! usage ledger as ACP's `usage_update` and `/spend` read it, so it counts
//! turns before a daemon restart and every provider request (retries,
//! compaction, titles, subagents) once.

use rho_protocol::SessionEvent as WireEvent;

use crate::usage::report::Valuation;

/// `events`, one runtime event's wire events, with `session`'s cost so far
/// on their usage event: one runtime event maps to at most one. Reads the
/// ledger only when there is one.
pub(super) async fn priced(
    mut events: Vec<WireEvent>,
    session: &rho_sdk::SessionId,
) -> Vec<WireEvent> {
    let Some(WireEvent::Usage {
        cost_usd_micros, ..
    }) = events
        .iter_mut()
        .find(|event| matches!(event, WireEvent::Usage { .. }))
    else {
        return events;
    };
    *cost_usd_micros = usd_micros(crate::usage::session_valuation(session).await);
    events
}

/// What `valuation` comes to in millionths of a dollar once a request had
/// a price; none, never 0, otherwise, as for a session that ran only local
/// models (the TUI statusline shows none either).
fn usd_micros(valuation: Valuation) -> Option<u64> {
    match valuation {
        Valuation::Usd(micros) => Some(micros),
        Valuation::Local | Valuation::Unpriced => None,
    }
}

#[cfg(test)]
#[path = "cost_tests.rs"]
mod tests;
