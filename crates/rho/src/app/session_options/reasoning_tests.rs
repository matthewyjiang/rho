use pretty_assertions::assert_eq;
use rho_protocol as wire;
use rho_providers::reasoning::ReasoningLevel;

use super::reasoning_options;
use crate::config::Config;

fn config(provider: &str, model: &str) -> Config {
    Config {
        provider: provider.into(),
        model: model.into(),
        ..Config::default()
    }
}

// Covers: a model the catalog does not know offers every level at the
// session's current one, and a model whose reasoning is fixed offers none,
// so hosts render no picker that can never change.
// Owner: session options
#[test]
fn reasoning_options_follow_the_model_capabilities() {
    assert_eq!(
        reasoning_options(&config("test", "model"), ReasoningLevel::High),
        Some(wire::ReasoningOptions {
            current: wire::ReasoningLevel::High,
            available: vec![
                wire::ReasoningLevel::Off,
                wire::ReasoningLevel::Minimal,
                wire::ReasoningLevel::Low,
                wire::ReasoningLevel::Medium,
                wire::ReasoningLevel::High,
                wire::ReasoningLevel::Xhigh,
                wire::ReasoningLevel::Max,
            ],
        })
    );
    assert_eq!(
        reasoning_options(&config("github-copilot", "gpt-4.1"), ReasoningLevel::High),
        None
    );
}
