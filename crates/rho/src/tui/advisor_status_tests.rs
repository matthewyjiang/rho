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
            (None, "off", "off", false),
        ),
        (
            AdvisorStatus::new(/*advisor_mode*/ false, Some(&model())),
            (None, "off", "off", false),
        ),
        (
            AdvisorStatus::new(/*advisor_mode*/ true, None),
            (
                Some("advisor: no model"),
                "on · no model",
                "on, but no advisor model is selected",
                true,
            ),
        ),
        (
            AdvisorStatus::new(/*advisor_mode*/ true, Some(&model())),
            (
                Some("advisor: anthropic/claude-fable-5"),
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
                Some("advisor: claude-code/opus"),
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
                Some("advisor: claude-code/default"),
                "on · claude-code/default",
                "on, claude-code/default reviews the session",
                false,
            ),
        ),
    ];

    for (status, (statusline, badge, detail, needs_model)) in cases {
        assert_eq!(
            (
                status.statusline_text().as_deref(),
                status.badge().as_str(),
                status.detail().as_str(),
                status.needs_model(),
            ),
            (statusline, badge, detail, needs_model),
            "{status:?}"
        );
    }
}
