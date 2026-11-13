use pretty_assertions::assert_eq;
use std::time::{Duration, Instant};

use super::{McpInFlightCalls, McpRouteError};

// Covers: a finished tool call must stop being a route for server-initiated
// requests, so a late elicitation cannot be answered by a call that already
// returned.
// Owner: MCP in-flight call registry.
#[test]
fn registration_is_withdrawn_when_the_call_ends() {
    let calls = McpInFlightCalls::new();
    let (registration, _questions) = calls.register();
    assert!(calls.sole_caller().is_ok());

    drop(registration);

    assert_eq!(
        calls.sole_caller().err(),
        Some(McpRouteError::NoCallInFlight)
    );
}

// Covers: concurrent calls must stay individually addressable, so withdrawing
// one never withdraws another. A recent peer release still fails closed for
// nested routing, because a delayed request from the finished call could arrive
// for the survivor.
// Owner: MCP in-flight call registry.
#[test]
fn concurrent_calls_are_withdrawn_independently() {
    let calls = McpInFlightCalls::new();
    let (first, _first_questions) = calls.register();
    let (second, _second_questions) = calls.register();
    assert_eq!(
        calls.sole_caller().err(),
        Some(McpRouteError::AmbiguousCall { in_flight: 2 })
    );

    drop(first);

    assert_eq!(
        calls.sole_caller().err(),
        Some(McpRouteError::AttributionUncertain)
    );
    drop(second);
    assert_eq!(
        calls.sole_caller().err(),
        Some(McpRouteError::NoCallInFlight)
    );
}

// Covers: after call A ends and call B starts, nested requests must not route
// to B while a delayed message from A could still arrive.
// Owner: MCP in-flight call registry.
#[test]
fn sequential_calls_fail_closed() {
    let calls = McpInFlightCalls::new();
    let (first, _first_questions) = calls.register();
    assert!(calls.sole_caller().is_ok());

    drop(first);

    let (_second, _second_questions) = calls.register();
    assert_eq!(
        calls.sole_caller().err(),
        Some(McpRouteError::AttributionUncertain)
    );
}

// Covers: once the grace window after a release has elapsed, a later sole call
// may own nested requests again.
// Owner: MCP in-flight call registry.
#[test]
fn attribution_recovers_after_grace() {
    let calls = McpInFlightCalls::new();
    let (first, _first_questions) = calls.register();
    drop(first);

    let (_second, _second_questions) = calls.register();
    // Simulate the grace window having elapsed without sleeping on the clock.
    calls.set_last_release_at_for_test(Instant::now() - Duration::from_secs(6));
    assert!(calls.sole_caller().is_ok());
}
