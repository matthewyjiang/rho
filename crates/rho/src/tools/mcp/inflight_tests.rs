use pretty_assertions::assert_eq;

use rho_sdk::CancellationToken;

use super::{McpInFlightCalls, McpRouteError};

// Covers: a finished tool call must stop being a route for server-initiated
// requests, so a late elicitation cannot be answered by a call that already
// returned.
// Owner: MCP in-flight call registry.
#[test]
fn registration_is_withdrawn_when_the_call_ends() {
    let calls = McpInFlightCalls::new();
    let (registration, _questions) = calls.register(CancellationToken::new());
    assert!(calls.sole_caller().is_ok());

    drop(registration);

    assert_eq!(
        calls.sole_caller().err(),
        Some(McpRouteError::NoCallInFlight)
    );
}

// Covers: concurrent calls must stay individually addressable, so withdrawing
// one never withdraws another.
// Owner: MCP in-flight call registry.
#[test]
fn concurrent_calls_are_withdrawn_independently() {
    let calls = McpInFlightCalls::new();
    let (first, _first_questions) = calls.register(CancellationToken::new());
    let (second, _second_questions) = calls.register(CancellationToken::new());
    assert_eq!(
        calls.sole_caller().err(),
        Some(McpRouteError::AmbiguousCall { in_flight: 2 })
    );

    drop(first);

    assert!(calls.sole_caller().is_ok());
    drop(second);
    assert_eq!(
        calls.sole_caller().err(),
        Some(McpRouteError::NoCallInFlight)
    );
}
