use pretty_assertions::assert_eq;

use super::parse_usage_screen;

const NOW: i64 = 1_700_000_000;

fn window_map(text: &str) -> Vec<(String, Option<f64>, bool)> {
    let state = parse_usage_screen(text, NOW).expect("parsed");
    state
        .sorted_windows()
        .into_iter()
        .map(|window| {
            (
                window.info.window_key().to_owned(),
                window.info.utilization,
                window.info.resets_at.is_some(),
            )
        })
        .collect()
}

// Covers: /usage panel windows must map onto stream rate-limit keys with used%.
// Owner: pure unit
#[test]
fn typical_usage_panel_maps_session_week_and_extra() {
    let text = r#"
Settings:   Status    Config   [Usage]
Current session
████████░░░░░░░░  1% used
Resets in 3h
Current week (all models)
░░░░░░░░░░░░░░░░  0% used
Resets in 2d
Current week (Sonnet only)
░░░░░░░░░░░░░░░░  0% used
Resets in 3h
Extra usage
██░░░░░░░░░░░░░░  15% used
$77.33 / $500.00 spent · Resets in 14d
"#;
    assert_eq!(
        window_map(text),
        vec![
            ("five_hour".into(), Some(0.01), true),
            ("seven_day".into(), Some(0.0), true),
            ("seven_day_sonnet".into(), Some(0.0), true),
            ("extra_usage".into(), Some(0.15), true),
        ]
    );
}

#[test]
fn relative_reset_in_hours_and_minutes_becomes_unix() {
    let text = "Current session\n10% used\nResets in 1h 30m\n";
    let state = parse_usage_screen(text, NOW).expect("parsed");
    let info = &state.sorted_windows()[0].info;
    assert_eq!(info.resets_at, Some(NOW + 5_400));
}

#[test]
fn header_without_percent_is_skipped() {
    let text = "Current session\nsome random text\nmore random text\n";
    assert!(parse_usage_screen(text, NOW).is_none());
}

#[test]
fn box_drawing_and_leading_whitespace_are_stripped() {
    let text = "│   Current session   │\n│   ██░░  10% used   │\n│   Resets in 5m     │\n";
    assert_eq!(
        window_map(text),
        vec![("five_hour".into(), Some(0.10), true)]
    );
}

// Covers: Fable must parse from Claude's parenthetical and from a 3-card row.
// Owner: pure unit
#[test]
fn fable_week_parses_from_live_layouts() {
    let same_line = "Current session  0%used  Current week (all models)  18%used  Current week (Claude Fable)  33%used\n";
    assert_eq!(
        window_map(same_line)
            .into_iter()
            .map(|(key, used, _)| (key, used))
            .collect::<Vec<_>>(),
        vec![
            ("five_hour".into(), Some(0.0)),
            ("seven_day".into(), Some(0.18)),
            ("seven_day_fable".into(), Some(0.33)),
        ]
    );

    let columns = "\
Current session          Current week (all models)    Current week (Fable)
0%used                   18%used                      33%used
Resets in 3h             Resets in 2d                 Resets in 2d
";
    assert_eq!(
        window_map(columns)
            .into_iter()
            .map(|(key, used, _)| (key, used))
            .collect::<Vec<_>>(),
        vec![
            ("five_hour".into(), Some(0.0)),
            ("seven_day".into(), Some(0.18)),
            ("seven_day_fable".into(), Some(0.33)),
        ]
    );
}

// Covers: a Fable header without %used is still a named window the probe must wait for.
// Owner: pure unit
#[test]
fn named_window_keys_include_headers_without_percent() {
    let keys = super::named_window_keys(
        "Current session\n0%used\nCurrent week (all models)\nCurrent week (Fable)\n",
    );
    assert_eq!(keys, vec!["five_hour", "seven_day", "seven_day_fable"]);
}
