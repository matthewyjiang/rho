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

// Covers: a session offers Auto only once a permission classifier model is
// configured, or while it already runs Auto, so a host never offers a mode
// the session refuses.
// Owner: session options
#[test]
fn auto_is_offered_only_with_a_classifier_model() {
    let current = CurrentModel {
        provider: "openai".into(),
        model: "gpt-5.5".into(),
        auth: "api-key".into(),
    };
    let classifier = crate::config::Config::parse_settings(
        "[internal_agents.permission-classifier]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nauth = \"api-key\"\n",
    )
    .unwrap();
    let offers_auto = |config: &Config, mode: PermissionMode| {
        build(
            &current,
            config,
            /*available_auths*/ &[],
            ReasoningLevel::High,
            wire::FastModeOptions { enabled: false },
            mode,
            Vec::new(),
        )
        .permission_mode
        .available
        .contains(&PermissionMode::Auto.into())
    };

    assert_eq!(
        [
            offers_auto(&Config::default(), PermissionMode::Supervised),
            offers_auto(&classifier, PermissionMode::Supervised),
            offers_auto(&Config::default(), PermissionMode::Auto),
        ],
        [false, true, true]
    );
}
