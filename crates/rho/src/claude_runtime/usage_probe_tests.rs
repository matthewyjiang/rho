use super::{
    classify_idle_screen, classify_usage_screen, trust_yes_selected, waiting_on_named_windows,
    IdleScreen, UsageScreen,
};

// Covers: host renderer overrides must not change the probe's mode, while
// endpoint environment survives and host terminal markers are removed.
// Owner: environment policy
#[test]
fn probe_environment_isolates_renderer_preferences() {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;

    let expected = BTreeMap::from([
        ("HTTPS_PROXY".into(), "http://proxy.invalid".into()),
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
        ("CLAUDE_CODE_AUTO_CONNECT_IDE".into(), "false".into()),
        ("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "1".into()),
        ("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN".into(), "1".into()),
        ("CLAUDE_CODE_NO_FLICKER".into(), "0".into()),
    ]);
    for renderer in [None, Some(("0", "1")), Some(("1", "0"))] {
        let mut inherited = vec![
            ("HTTPS_PROXY".into(), "http://proxy.invalid".into()),
            ("TERM".into(), "dumb".into()),
            ("TERM_PROGRAM".into(), "host-terminal".into()),
            ("HERDR_ENV".into(), "1".into()),
        ];
        if let Some((disable, fullscreen)) = renderer {
            inherited.extend([
                (
                    "CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN".into(),
                    disable.into(),
                ),
                ("CLAUDE_CODE_NO_FLICKER".into(), fullscreen.into()),
            ]);
        }
        assert_eq!(
            super::claude_probe_env(inherited.into_iter())
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
            expected
        );
    }
}

// Covers: the trust dialog's ❯ must not be treated as the idle prompt.
// Owner: pure unit
#[test]
fn trust_dialog_is_not_a_ready_prompt() {
    let screen = r#"
Accessing workspace:
Quick safety check
❯ No, exit
Yes, I trust this folder
Enter to confirm
"#;
    assert_eq!(classify_idle_screen(screen), IdleScreen::Trust);
}

// Covers: Enter on the default No row must not be treated as Yes.
// Owner: pure unit
#[test]
fn trust_yes_requires_the_pointer_on_yes() {
    let no = r#"
Accessing workspace:
Yes, I trust this folder
❯ No, exit
Do you trust this folder?
"#;
    assert!(!trust_yes_selected(no));
    let yes = r#"
Accessing workspace:
❯ Yes, I trust this folder
No, exit
Do you trust this folder?
"#;
    assert!(trust_yes_selected(yes));
}

// Covers: session+week paint must not finish while Fable is named without %.
// Owner: pure unit
#[test]
fn waits_while_named_fable_header_has_no_percent() {
    let partial =
        super::super::usage_parse::parse_usage_screen("Current session\n0%used\nResets in 1h\n", 0);
    assert!(waiting_on_named_windows(
        "Current session\n0%used\nCurrent week (Fable)\n",
        partial.as_ref()
    ));
    let complete = super::super::usage_parse::parse_usage_screen(
        "Current session\n0%used\nResets in 1h\nCurrent week (Fable)\n33%used\nResets in 2d\n",
        0,
    );
    assert!(!waiting_on_named_windows(
        "Current session\n0%used\nCurrent week (Fable)\n33%used\n",
        complete.as_ref()
    ));
}

// Covers: every /usage frame kind maps to the right decision, and failure
// footers win over visible percentages. Marker coverage lives here, not in
// per-string PTY runs.
// Owner: pure unit
#[test]
fn usage_screen_classification() {
    let windows = "Current session\n10% used\nCurrent week (all models)\n20% used\n";
    let cases: Vec<(&str, String, &str)> = vec![
        ("prompt", "? for shortcuts\n❯ /usage".into(), "NoPanel"),
        ("spinner", format!("{windows}Refreshing…\n"), "Refreshing"),
        (
            "named window without percent",
            format!("{windows}Current week (Fable)\n"),
            "Incomplete",
        ),
        ("complete", format!("{windows}Esc to cancel\n"), "Ready"),
        (
            "load error",
            format!("{windows}Failed to load usage data: response error\n"),
            "Failed",
        ),
        (
            "last-known fallback",
            format!("{windows}Showing last-known usage as of 2 minutes ago (could not refresh)\n"),
            "Failed",
        ),
        (
            "rate limited",
            format!("{windows}Showing last-known usage (rate limited — try again in a moment)\n"),
            "Failed",
        ),
        (
            "partial",
            format!("{windows}Partial usage data (rate limited — try again in a moment)\n"),
            "Failed",
        ),
        (
            "per-model unavailable",
            format!(
                "{windows}Per-model breakdown unavailable (rate limited — try again in a moment)\n"
            ),
            "Failed",
        ),
        (
            "could not refresh",
            format!("{windows}Could not refresh usage data\n"),
            "Failed",
        ),
        (
            "endpoint rate limited",
            format!("{windows}Usage endpoint is rate limited. Please try again in a moment.\n"),
            "Failed",
        ),
    ];
    let observed: Vec<(&str, &str)> = cases
        .iter()
        .map(|(name, screen, _)| {
            let kind = match classify_usage_screen(screen, 0) {
                UsageScreen::NoPanel => "NoPanel",
                UsageScreen::Failed => "Failed",
                UsageScreen::Refreshing => "Refreshing",
                UsageScreen::Incomplete => "Incomplete",
                UsageScreen::Ready(state) => {
                    assert_eq!(state.windows.len(), 2, "{name}");
                    "Ready"
                }
            };
            (*name, kind)
        })
        .collect();
    let expected: Vec<(&str, &str)> = cases.iter().map(|(name, _, kind)| (*name, *kind)).collect();
    pretty_assertions::assert_eq!(observed, expected);
}
