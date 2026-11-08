use pretty_assertions::assert_eq;
use rho_protocol as wire;
use rho_sdk::ReasoningLevel;

use super::{build, CurrentModel};
use crate::{config::Config, permission::PermissionMode};

fn fast_mode_of(provider: &str, model: &str, auth: &str) -> Option<wire::FastModeOptions> {
    let current = CurrentModel {
        provider: provider.into(),
        model: model.into(),
        auth: auth.into(),
    };
    let config = Config {
        provider: current.provider.clone(),
        model: current.model.clone(),
        auth: current.auth.clone(),
        ..Config::default()
    };
    build(
        &current,
        &config,
        /*available_auths*/ &[],
        ReasoningLevel::High,
        wire::FastModeOptions { enabled: true },
        PermissionMode::Supervised,
        Vec::new(),
    )
    .fast_mode
}

// Covers: fast mode is offered only for a model and auth that implement it,
// with the session's saved preference as its state, so a host never shows a
// toggle the runtime refuses to turn on.
// Owner: session options
#[test]
fn fast_mode_is_available_only_for_fast_capable_models() {
    let on = Some(wire::FastModeOptions { enabled: true });
    let cases = [
        (("openai-codex", "gpt-5.5", "codex"), on.clone()),
        (("xai", "grok-4.7", "xai-oauth"), on),
        (("xai", "grok-4.7", "api-key"), None),
        (("openai-codex", "gpt-5.4", "codex"), None),
    ];
    for ((provider, model, auth), expected) in cases {
        assert_eq!(
            fast_mode_of(provider, model, auth),
            expected,
            "{provider}/{model} via {auth}"
        );
    }
}
