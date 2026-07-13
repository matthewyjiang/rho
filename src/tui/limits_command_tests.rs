use pretty_assertions::assert_eq;

use super::*;

#[test]
fn renders_only_available_windows_with_remaining_bar() {
    let lines = usage_limit_lines(
        &ProviderLimits {
            provider: "Codex".into(),
            windows: vec![UsageLimitWindow {
                label: "Weekly".into(),
                remaining_percent: 69.0,
                resets_at_unix: now_unix() + 2 * 60 * 60 + 14 * 60,
            }],
        },
        80,
    );
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(text[0], "OAuth usage limits");
    assert_eq!(text[2], "Codex");
    assert!(text[3].contains("Weekly"));
    assert!(text[3].contains("███████░░░"));
    assert!(text[3].contains("69% left"));
    assert!(text[3].contains("resets in 2h 14m"));
    assert!(!text.join("\n").contains("5-hour"));
}

#[test]
fn narrow_layout_wraps_reset_instead_of_hiding_it() {
    let lines = usage_limit_window_lines(
        &UsageLimitWindow {
            label: "Weekly".into(),
            remaining_percent: 93.0,
            resets_at_unix: 10_000,
        },
        6,
        43,
        10_000 - 2 * 60 * 60 - 14 * 60,
    );
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_eq!(
        text,
        vec![
            "  Weekly   █████████░  93% left".to_string(),
            "  resets in 2h 14m".to_string(),
        ]
    );
}

#[test]
fn formats_reset_relative_only_within_one_day() {
    let window = UsageLimitWindow {
        label: "Weekly".into(),
        remaining_percent: 50.0,
        resets_at_unix: 200_000,
    };
    assert_eq!(format_reset(&window, 200_000 - 90 * 60), "in 1h 30m");
    assert!(!format_reset(&window, 0).starts_with("in "));
}
