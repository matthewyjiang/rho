//! The session's cost on its usage events (#1670), read from the durable
//! usage ledger as ACP's `usage_update` and `/spend` read it, so it counts
//! turns before a daemon restart and every provider request (retries,
//! compaction, titles, subagents) once.
//!
//! The ledger gains a request only once it completes, so while a response
//! streams, [`TurnCost`] adds what the provider has counted of it so far.

use rho_protocol::SessionEvent as WireEvent;
use rho_sdk::{model::ModelUsage, RunEvent};

use crate::{
    app::{
        session_events::FeedEvent,
        session_runtime::{CurrentModel, SessionEvent},
    },
    usage::{
        pricing::{catalog_cost_usd_micros, usage_difference},
        report::{catalog_route_pricing, RoutePricing, Valuation},
    },
};

/// One turn's pricing of the session's usage events: the ledger's total at
/// each model step and at the run's end, plus, between those, the response
/// under way.
pub(super) struct TurnCost {
    /// The catalog price of the model the turn runs on, which values
    /// usage the provider put no cost on.
    pricing: RoutePricing,
    /// The session's cost as the ledger last had it.
    ledger: Option<u64>,
    /// The run's usage when the ledger was last read: every request it
    /// counts has completed, so the ledger holds it.
    recorded: ModelUsage,
    /// The run's usage as the provider last counted it.
    run: ModelUsage,
    /// The most the session has been reported to cost since the ledger was
    /// last read. A response the provider discards leaves the
    /// run's usage, though the ledger will count it, so the cost holds
    /// here rather than fall until the next read.
    shown: Option<u64>,
}

impl TurnCost {
    /// A turn on `model`. Looks its catalog price up once: a stale row is
    /// fine for an estimate the ledger settles.
    pub(super) fn on(model: &CurrentModel) -> Self {
        Self::priced_by(catalog_route_pricing(&model.provider, &model.model))
    }

    /// A turn on a model the catalog has no price for.
    #[cfg(test)]
    pub(super) fn unpriced() -> Self {
        Self::priced_by(RoutePricing::Unknown)
    }

    fn priced_by(pricing: RoutePricing) -> Self {
        Self {
            pricing,
            ledger: None,
            recorded: ModelUsage::default(),
            run: ModelUsage::default(),
            shown: None,
        }
    }

    /// `events`, the wire events of the turn's `event`, with the session's
    /// cost so far on their usage event. Reads the ledger only for a usage
    /// event that no response is streaming under.
    pub(super) async fn priced(
        &mut self,
        event: &SessionEvent,
        mut events: Vec<FeedEvent>,
        session: &rho_sdk::SessionId,
    ) -> Vec<FeedEvent> {
        let streamed = match event {
            SessionEvent::Run(RunEvent::UsageUpdated { usage }) => Some(self.streaming(usage)),
            _ => None,
        };
        let Some(cost) = usage_cost(events.iter_mut().map(|event| &mut event.event)) else {
            return events;
        };
        *cost = match streamed {
            Some(streamed) => streamed,
            None => {
                let ledger = usd_micros(crate::usage::session_valuation(session).await);
                self.read_ledger(ledger);
                ledger
            }
        };
        events
    }

    /// The session's cost with the response under way at `run`, the run's
    /// usage so far: the ledger's total plus what the run used since.
    fn streaming(&mut self, run: &ModelUsage) -> Option<u64> {
        self.run = run.clone();
        let unrecorded = usage_difference(run, Some(&self.recorded));
        let unrecorded = unrecorded.cost_usd_micros.or_else(|| match &self.pricing {
            RoutePricing::Catalog(metadata) => catalog_cost_usd_micros(&unrecorded, metadata),
            RoutePricing::Local | RoutePricing::Unknown => None,
        });
        let cost = match (self.ledger, unrecorded) {
            (None, None) => None,
            (ledger, unrecorded) => Some(
                ledger
                    .unwrap_or_default()
                    .saturating_add(unrecorded.unwrap_or_default()),
            ),
        };
        self.shown = self.shown.max(cost);
        self.shown
    }

    /// Takes `ledger` as the session's cost with every request the run has
    /// made so far in it.
    fn read_ledger(&mut self, ledger: Option<u64>) {
        self.ledger = ledger;
        self.recorded = self.run.clone();
        self.shown = None;
    }
}

/// The cost of the usage event among `events`, to set: one runtime event
/// maps to at most one.
fn usage_cost<'a>(
    events: impl IntoIterator<Item = &'a mut WireEvent>,
) -> Option<&'a mut Option<u64>> {
    events.into_iter().find_map(|event| match event {
        WireEvent::Usage {
            cost_usd_micros, ..
        } => Some(cost_usd_micros),
        _ => None,
    })
}

/// `events`, one runtime event's wire events outside a turn's run, with
/// `session`'s cost so far on their usage event. Reads the ledger only when
/// there is one.
pub(super) async fn priced(
    mut events: Vec<WireEvent>,
    session: &rho_sdk::SessionId,
) -> Vec<WireEvent> {
    if let Some(cost) = usage_cost(&mut events) {
        *cost = usd_micros(crate::usage::session_valuation(session).await);
    }
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
