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

// Covers: "Resets Sep 5, 8am" must land on that day, not today-or-tomorrow.
// Weekly resets are at most 7 days out, so the day-of-month pins the date.
// The month token is only an anchor for the day number - its value is not
// validated, so the literal "Sep" here pairs fine with a November NOW.
// Owner: pure unit
#[test]
fn dated_reset_lands_on_the_named_day() {
    use chrono::{Datelike, TimeZone, Timelike};

    let now = chrono::Local.timestamp_opt(NOW, 0).unwrap();
    let target = now + chrono::TimeDelta::days(4);
    let text = format!(
        "Current week (all models)\n26% used\nResets Sep {}, 8am (America/Los_Angeles)\n",
        target.day()
    );
    let state = parse_usage_screen(&text, NOW).expect("parsed");
    let resets_at = state.sorted_windows()[0].info.resets_at.expect("resets_at");
    let resolved = chrono::Local.timestamp_opt(resets_at, 0).unwrap();
    assert_eq!(
        (resolved.day(), resolved.hour(), resolved.minute()),
        (target.day(), 8, 0)
    );
}

// Covers: a clock-only reset ("Resets 5:30am") still resolves today-or-tomorrow
// and must not misread the hour as a day-of-month.
// Owner: pure unit
#[test]
fn clock_only_reset_stays_within_a_day() {
    let text = "Current session\n29% used\nResets 5:30am (America/Los_Angeles)\n";
    let state = parse_usage_screen(text, NOW).expect("parsed");
    let resets_at = state.sorted_windows()[0].info.resets_at.expect("resets_at");
    let delta = resets_at - NOW;
    assert!((0..=86_400).contains(&delta), "{delta}");
}

// Covers: digits inside a tz label ("(UTC+10)", "(EST5EDT)") must not be read
// as a day-of-month and push a clock-only reset days into the future.
// Drive from early in the month so a misread 5 or 10 falls inside the nine-day
// search window (NOW is Nov 14, which excludes both).
// Owner: pure unit
#[test]
fn tz_label_digits_are_not_a_day_of_month() {
    const EARLY_IN_MONTH: i64 = 1_698_926_400; // 2023-11-02T12:00:00Z
    for label in ["(UTC+10)", "(EST5EDT)", "(GMT-5)"] {
        let text = format!("Current session\n29% used\nResets 5:30am {label}\n");
        let state = parse_usage_screen(&text, EARLY_IN_MONTH).expect("parsed");
        let resets_at = state.sorted_windows()[0].info.resets_at.expect("resets_at");
        let delta = resets_at - EARLY_IN_MONTH;
        assert!((0..=86_400).contains(&delta), "{label}: {delta}");
    }
}

#[test]
fn header_without_percent_is_skipped() {
    let text = "Current session\nsome random text\nmore random text\n";
    assert!(parse_usage_screen(text, NOW).is_none());
}

// Covers: a bare % above the usage bar must not be taken as the window used%.
// Owner: pure unit
#[test]
fn incidental_percent_is_not_window_used() {
    let compact = "Current session\nauto-compact at 80%\n10% used\nResets in 1h\n";
    assert_eq!(
        window_map(compact),
        vec![("five_hour".into(), Some(0.10), true)]
    );
    let only_bare = "Current session\nauto-compact at 80%\nResets in 1h\n";
    assert!(parse_usage_screen(only_bare, NOW).is_none());
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
