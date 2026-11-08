use pretty_assertions::assert_eq;
use rho_protocol::{Cost, SessionEvent as WireEvent};

use super::with_valuation;
use crate::usage::report::Valuation;

fn usage(cost: Option<Cost>) -> WireEvent {
    WireEvent::Usage {
        used_tokens: 100,
        window_tokens: Some(1_000),
        cost,
    }
}

fn notice() -> WireEvent {
    WireEvent::Notice {
        text: "saved".into(),
    }
}

// Covers: usage carries the session's cost in dollars once a request had a
// price, and no cost (never 0) while none has or every request ran
// locally; other events are left as they are.
// Owner: session event usage pricing.
#[test]
fn usage_carries_the_session_cost_once_a_request_had_a_price() {
    let dollars = Some(Cost {
        amount_micros: 1_250_000,
        currency: "USD".into(),
    });
    let cases = [
        (Valuation::Usd(1_250_000), dollars),
        (Valuation::Local, None),
        (Valuation::Unpriced, None),
    ];
    for (valuation, cost) in cases {
        assert_eq!(
            with_valuation(vec![usage(None), notice()], valuation),
            vec![usage(cost), notice()],
            "{valuation:?}"
        );
    }
}
