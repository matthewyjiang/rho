use pretty_assertions::assert_eq;

use super::usd_micros;
use crate::usage::report::Valuation;

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
