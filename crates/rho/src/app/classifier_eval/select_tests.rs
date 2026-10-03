use pretty_assertions::assert_eq;

use super::{select_model, DECISION_SCREEN_ID};
use crate::config::{Config, InternalAgentModelConfig};

// Covers: overriding an entry's model keeps its configured auth while the
// provider stays the same, so an eval against an authenticated server does
// not silently fall back; a new provider takes that provider's default auth.
// The screen's allow threshold is kept either way, so an eval runs at the
// configured threshold.
// Owner: classifier eval model selection.
#[test]
fn model_override_keeps_auth_only_on_the_same_provider() {
    let cases = [
        (
            "ollama/clef-flash",
            ("ollama", "clef-flash", "ollama-api-key", Some(100)),
        ),
        (
            "anthropic/claude-haiku-4-5",
            (
                "anthropic",
                "claude-haiku-4-5",
                "anthropic-api-key",
                Some(100),
            ),
        ),
    ];
    for (reference, expected) in cases {
        let mut config = Config::default();
        let mut screen =
            InternalAgentModelConfig::new("ollama".into(), "clef".into(), "ollama-api-key".into());
        screen.expect_rho_mut().allow_threshold_percent = Some(100);
        config.set_internal_agent_model_config(DECISION_SCREEN_ID, screen);

        select_model(&mut config, DECISION_SCREEN_ID, reference).unwrap();

        let selection = config
            .internal_agent_model(DECISION_SCREEN_ID)
            .and_then(InternalAgentModelConfig::rho)
            .unwrap();
        let actual = (
            selection.provider.as_str(),
            selection.model.as_str(),
            selection.auth.as_str(),
            selection.allow_threshold_percent,
        );
        assert_eq!(actual, expected, "{reference}");
    }
}
