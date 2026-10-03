use pretty_assertions::assert_eq;

use super::{select_model, DECISION_SCREEN_ID};
use crate::config::{Config, InternalAgentModelConfig};

// Covers: overriding an entry's model keeps its configured auth while the
// provider stays the same, so an eval against an authenticated server does
// not silently fall back; a new provider takes that provider's default auth.
// Owner: classifier eval model selection.
#[test]
fn model_override_keeps_auth_only_on_the_same_provider() {
    let cases = [
        (
            "ollama/clef-flash",
            ("ollama", "clef-flash", "ollama-api-key"),
        ),
        (
            "anthropic/claude-haiku-4-5",
            ("anthropic", "claude-haiku-4-5", "anthropic-api-key"),
        ),
    ];
    for (reference, expected) in cases {
        let mut config = Config::default();
        config.set_internal_agent_model_config(
            DECISION_SCREEN_ID,
            InternalAgentModelConfig::new("ollama".into(), "clef".into(), "ollama-api-key".into()),
        );

        select_model(&mut config, DECISION_SCREEN_ID, reference).unwrap();

        let selection = config
            .internal_agent_model(DECISION_SCREEN_ID)
            .and_then(InternalAgentModelConfig::rho)
            .unwrap();
        let actual = (
            selection.provider.as_str(),
            selection.model.as_str(),
            selection.auth.as_str(),
        );
        assert_eq!(actual, expected, "{reference}");
    }
}
