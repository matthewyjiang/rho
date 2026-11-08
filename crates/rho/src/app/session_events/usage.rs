//! The session's cost on its usage events (#1670), read from the durable
//! usage ledger as ACP's `usage_update` and `/spend` read it, so it counts
//! turns before a daemon restart and every provider request (retries,
//! compaction, titles, subagents) once.

use rho_protocol::{Cost, SessionEvent as WireEvent};

use crate::usage::report::Valuation;

/// `events` with `session`'s cost so far on each usage event. Reads the
/// ledger only when there is a usage event to price.
pub(crate) async fn priced(events: Vec<WireEvent>, session: &rho_sdk::SessionId) -> Vec<WireEvent> {
    if !events
        .iter()
        .any(|event| matches!(event, WireEvent::Usage { .. }))
    {
        return events;
    }
    with_valuation(events, crate::usage::session_valuation(session).await)
}

/// `events` with each usage event costing what `valuation` says: dollars
/// once a request had a price, else no cost, never 0, as for a session that
/// ran only local models (the TUI statusline shows none either).
fn with_valuation(mut events: Vec<WireEvent>, valuation: Valuation) -> Vec<WireEvent> {
    let priced = match valuation {
        Valuation::Usd(micros) => Some(Cost {
            amount_micros: micros,
            currency: "USD".to_owned(),
        }),
        Valuation::Local | Valuation::Unpriced => None,
    };
    for event in &mut events {
        if let WireEvent::Usage { cost, .. } = event {
            cost.clone_from(&priced);
        }
    }
    events
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
