use pretty_assertions::assert_eq;

use super::*;

fn model() -> InternalAgentModelConfig {
    InternalAgentModelConfig::new(
        "anthropic".into(),
        "claude-fable-5".into(),
        "api-key".into(),
    )
}

// Covers: advisor mode on with no model must never read as working, and off must
// stay out of the way regardless of a configured model.
// Owner: advisor status presentation.
#[test]
fn advisor_mode_and_model_map_to_one_of_three_states() {
    let reviewing = |model: &str| AdvisorStatus::Reviewing {
        model: model.into(),
    };
    for (case, status, expected, needs_model) in [
        (
            "off without model",
            AdvisorStatus::new(/*advisor_mode*/ false, None),
            AdvisorStatus::Off,
            false,
        ),
        (
            "off with model",
            AdvisorStatus::new(/*advisor_mode*/ false, Some(&model())),
            AdvisorStatus::Off,
            false,
        ),
        (
            "on without model",
            AdvisorStatus::new(/*advisor_mode*/ true, None),
            AdvisorStatus::MissingModel,
            true,
        ),
        (
            "on with model",
            AdvisorStatus::new(/*advisor_mode*/ true, Some(&model())),
            reviewing("anthropic/claude-fable-5"),
            false,
        ),
        (
            "on with claude cli model",
            AdvisorStatus::new(
                /*advisor_mode*/ true,
                Some(&InternalAgentModelConfig::claude_cli(Some("opus".into()))),
            ),
            reviewing("claude-code/opus"),
            false,
        ),
        (
            "on with default claude cli model",
            AdvisorStatus::new(
                /*advisor_mode*/ true,
                Some(&InternalAgentModelConfig::claude_cli(None)),
            ),
            reviewing("claude-code/default"),
            false,
        ),
    ] {
        assert_eq!(status.needs_model(), needs_model, "{case}");
        assert_eq!(status, expected, "{case}");
    }
}
