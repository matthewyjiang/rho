use pretty_assertions::assert_eq;

use rho_sdk::model::ModelUsage;

use super::{usd_micros, TurnCost};
use crate::{app::session_runtime::CurrentModel, usage::report::Valuation};

/// A turn on a model the catalog has no price for.
fn turn() -> TurnCost {
    TurnCost::new(CurrentModel {
        provider: "unlisted".into(),
        model: "unlisted".into(),
        auth: "unlisted".into(),
    })
}

/// The run's usage once the provider has billed it `micros` in all.
fn billed(micros: u64) -> ModelUsage {
    ModelUsage {
        output_tokens: Some(10),
        cost_usd_micros: Some(micros),
        ..ModelUsage::default()
    }
}

// Covers: while a response streams, the session costs what the ledger had
// plus what the provider has billed the run since; once the ledger holds a
// finished response, the next one adds only its own.
// Owner: daemon session usage pricing.
#[test]
fn a_streaming_response_adds_to_the_ledger_only_what_it_has_not_recorded() {
    let mut cost = turn();
    cost.read_ledger(Some(5_000_000));
    let first = [
        cost.streaming(&billed(1_000_000)),
        cost.streaming(&billed(2_000_000)),
    ];
    // The first response finished: the ledger has its two dollars.
    cost.read_ledger(Some(7_000_000));
    let second = cost.streaming(&billed(2_500_000));

    assert_eq!(
        (first, second),
        ([Some(6_000_000), Some(7_000_000)], Some(7_500_000))
    );
}

// Covers: usage nobody can price adds no cost: a session with no priced
// request still shows none, and one with a cost keeps it.
// Owner: daemon session usage pricing.
#[test]
fn unpriced_streaming_usage_leaves_the_cost_as_the_ledger_had_it() {
    let unpriced = ModelUsage {
        output_tokens: Some(10),
        ..ModelUsage::default()
    };
    let mut cost = turn();
    let none = cost.streaming(&unpriced);
    cost.read_ledger(Some(5_000_000));

    assert_eq!((none, cost.streaming(&unpriced)), (None, Some(5_000_000)));
}

// Covers: usage carries the session's cost in dollars once a request had a
// price, and no cost (never 0) while none has or every request ran
// locally.
// Owner: daemon session usage pricing.
#[test]
fn usage_costs_dollars_once_a_request_had_a_price() {
    let cases = [
        (Valuation::Usd(1_250_000), Some(1_250_000)),
        (Valuation::Local, None),
        (Valuation::Unpriced, None),
    ];
    for (valuation, cost) in cases {
        assert_eq!(usd_micros(valuation), cost, "{valuation:?}");
    }
}
