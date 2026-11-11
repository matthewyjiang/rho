use pretty_assertions::assert_eq;

use super::*;
use crate::tui::model_names::tests::with_catalog_names;

fn model(model: &str) -> InternalAgentModelConfig {
    InternalAgentModelConfig::new("anthropic".into(), model.into(), "api-key".into())
}

// Covers: advisor mode on with no model must never read as working, and off must
// stay out of the way regardless of a configured model. A model reads by its
// catalog name when one is known.
// Owner: advisor status presentation.
#[test]
fn advisor_mode_and_model_map_to_one_of_three_states() {
    let names = ModelNames::default();
    let reviewing = |model: &str| AdvisorStatus::Reviewing {
        model: model.into(),
    };
    with_catalog_names(&[("anthropic", "claude-fable-5", "Claude Fable 5")], || {
        for (case, status, expected, needs_model) in [
            (
                "off without model",
                AdvisorStatus::new(/*advisor_mode*/ false, None, names),
                AdvisorStatus::Off,
                false,
            ),
            (
                "off with model",
                AdvisorStatus::new(
                    /*advisor_mode*/ false,
                    Some(&model("claude-fable-5")),
                    names,
                ),
                AdvisorStatus::Off,
                false,
            ),
            (
                "on without model",
                AdvisorStatus::new(/*advisor_mode*/ true, None, names),
                AdvisorStatus::MissingModel,
                true,
            ),
            (
                "on with named model",
                AdvisorStatus::new(
                    /*advisor_mode*/ true,
                    Some(&model("claude-fable-5")),
                    names,
                ),
                reviewing("Claude Fable 5"),
                false,
            ),
            (
                "on with unnamed model",
                AdvisorStatus::new(
                    /*advisor_mode*/ true,
                    Some(&model("claude-next")),
                    names,
                ),
                reviewing("anthropic/claude-next"),
                false,
            ),
            (
                "on with claude cli model",
                AdvisorStatus::new(
                    /*advisor_mode*/ true,
                    Some(&InternalAgentModelConfig::claude_cli(Some("opus".into()))),
                    names,
                ),
                reviewing("claude-code/opus"),
                false,
            ),
            (
                "on with default claude cli model",
                AdvisorStatus::new(
                    /*advisor_mode*/ true,
                    Some(&InternalAgentModelConfig::claude_cli(None)),
                    names,
                ),
                reviewing("claude-code/default"),
                false,
            ),
        ] {
            assert_eq!(status.needs_model(), needs_model, "{case}");
            assert_eq!(status, expected, "{case}");
        }
    });
}
