use super::*;
use rho_providers::model::ReasoningLevelSet;

fn exact() -> ReasoningCapabilities {
    ReasoningCapabilities::Levels(ReasoningLevelSet::new(vec![
        ReasoningLevel::Low,
        ReasoningLevel::High,
    ]))
}

// Covers: once model metadata arrives, a level changed during the fetch is
// rejected and restored, an explicit unsupported level stays rejected, an
// unchanged persisted level is normalized silently, and fixed models keep the
// global preference.
// Owner: tui reasoning metadata resolution
#[test]
fn explicit_change_during_fetch_is_rejected_and_restored() {
    use ReasoningRequestSource::{Explicit, PersistedOrDefault};
    let cases = [
        (
            "explicit change during fetch",
            exact(),
            ReasoningLevel::Medium,
            (ReasoningLevel::Low, PersistedOrDefault),
            ReasoningLevel::Low,
            Some(ReasoningLevel::Medium),
        ),
        (
            "initial explicit value",
            exact(),
            ReasoningLevel::Medium,
            (ReasoningLevel::Medium, Explicit),
            ReasoningLevel::High,
            Some(ReasoningLevel::Medium),
        ),
        (
            "unchanged persisted value",
            exact(),
            ReasoningLevel::Medium,
            (ReasoningLevel::Medium, PersistedOrDefault),
            ReasoningLevel::High,
            None,
        ),
        (
            "fixed model",
            ReasoningCapabilities::NotConfigurable,
            ReasoningLevel::High,
            (ReasoningLevel::High, PersistedOrDefault),
            ReasoningLevel::High,
            None,
        ),
    ];
    for (case, capabilities, current, requested, effective, rejected) in cases {
        let resolved = resolve_fetched_reasoning(&capabilities, current, Some(requested));
        assert_eq!(
            (resolved.effective, resolved.rejected),
            (effective, rejected),
            "{case}"
        );
    }
}
