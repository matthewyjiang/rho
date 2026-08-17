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
// stay out of the way, on every surface that shows the mode.
// Owner: advisor status presentation.
#[test]
fn each_advisor_state_reads_the_same_way_on_every_surface() {
    let cases = [
        (
            AdvisorStatus::new(/*advisor_mode*/ false, None),
            (Vec::new(), "off", "off", false),
        ),
        (
            AdvisorStatus::new(/*advisor_mode*/ false, Some(&model())),
            (Vec::new(), "off", "off", false),
        ),
        (
            AdvisorStatus::new(/*advisor_mode*/ true, None),
            (
                vec!["advisor: no model".into(), "advisor".into()],
                "on · no model",
                "on, but no advisor model is selected",
                true,
            ),
        ),
        (
            AdvisorStatus::new(/*advisor_mode*/ true, Some(&model())),
            (
                vec!["advisor: anthropic/claude-fable-5".into(), "advisor".into()],
                "on · anthropic/claude-fable-5",
                "on, anthropic/claude-fable-5 reviews the session",
                false,
            ),
        ),
        (
            AdvisorStatus::new(
                /*advisor_mode*/ true,
                Some(&InternalAgentModelConfig::claude_cli(Some("opus".into()))),
            ),
            (
                vec!["advisor: claude-code/opus".into(), "advisor".into()],
                "on · claude-code/opus",
                "on, claude-code/opus reviews the session",
                false,
            ),
        ),
        (
            AdvisorStatus::new(
                /*advisor_mode*/ true,
                Some(&InternalAgentModelConfig::claude_cli(None)),
            ),
            (
                vec!["advisor: claude-code/default".into(), "advisor".into()],
                "on · claude-code/default",
                "on, claude-code/default reviews the session",
                false,
            ),
        ),
    ];

    for (status, (labels, badge, detail, needs_model)) in cases {
        assert_eq!(
            (
                status.divider_labels(),
                status.badge().as_str(),
                status.detail().as_str(),
                status.needs_model(),
            ),
            (labels, badge, detail, needs_model),
            "{status:?}"
        );
    }
}
