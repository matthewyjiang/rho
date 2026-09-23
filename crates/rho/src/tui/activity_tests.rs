use std::time::Duration;

use pretty_assertions::assert_eq;

use super::*;

// Covers: rail rows keep identity · activity  elapsed when they fit, use the
// pane width instead of a 52-col clamp, drop activity before chopping
// identity, and stay within the pane on narrow hover trailing.
// Owner: pure layout
#[test]
fn rail_row_layout_assembles_columns() {
    let row_style = Theme::activity_rail();
    struct Case {
        name: &'static str,
        identity: &'static [&'static str],
        activity: &'static str,
        trailing: &'static str,
        width: usize,
        identity_intact: bool,
        activity_shown: bool,
        trailing_intact: bool,
    }
    let cases = [
        Case {
            name: "columns pack when they fit",
            identity: &["sleep", " ", "aaaaaaaa"],
            activity: "running",
            trailing: "4s",
            width: 80,
            identity_intact: true,
            activity_shown: true,
            trailing_intact: true,
        },
        Case {
            name: "long identity uses the pane width",
            identity: &[
                "◉ ",
                "explorer",
                "  ",
                "TUI Redundancy and Simplification Audit",
            ],
            activity: "read",
            trailing: "12s",
            width: 80,
            identity_intact: true,
            activity_shown: true,
            trailing_intact: true,
        },
        Case {
            name: "narrow row drops activity and keeps elapsed",
            identity: &["very-long-command"],
            activity: "running",
            trailing: "12s",
            width: 18,
            identity_intact: false,
            activity_shown: false,
            trailing_intact: true,
        },
        Case {
            name: "narrow hover trailing stays within the pane",
            identity: &["explorer"],
            activity: "read",
            trailing: "⏎ attach · 4s",
            width: 10,
            identity_intact: false,
            activity_shown: false,
            trailing_intact: false,
        },
    ];
    for case in cases {
        let line = RailRow {
            connector: tree_connector(true),
            identity: case
                .identity
                .iter()
                .map(|text| Span::styled(*text, row_style))
                .collect(),
            activity: case.activity.into(),
            activity_style: Theme::text(),
            trailing: case.trailing.into(),
            trailing_style: Theme::dim(),
            row_style,
        }
        .into_line(case.width);
        let texts: Vec<&str> = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let full: String = texts.concat();
        assert!(
            display_width(&full) <= case.width,
            "{}: {full:?} is {} cells",
            case.name,
            display_width(&full)
        );
        assert_eq!(
            (
                full.contains(&case.identity.concat()),
                texts.contains(&case.activity),
                full.trim_end().ends_with(case.trailing),
            ),
            (
                case.identity_intact,
                case.activity_shown,
                case.trailing_intact
            ),
            "{}: {full:?}",
            case.name
        );
    }
}

#[test]
fn bottom_follow_activity_inset_only_when_activity_and_pinned() {
    assert_eq!(bottom_follow_activity_inset(false, true), 0);
    assert_eq!(bottom_follow_activity_inset(true, false), 0);
    assert_eq!(
        bottom_follow_activity_inset(true, true),
        ACTIVITY_RAIL_ROWS + ACTIVITY_CONTENT_GAP_ROWS
    );
}

// Covers: the jump chip degrades full -> compact -> bare shortcut as width
// shrinks, for every attention state.
// Owner: pure unit (chip width degradation)
#[test]
fn jump_to_bottom_text_degrades_by_width() {
    let binding = "ctrl+e";
    let shortcut = format!("↓ {binding}");
    for state in [
        JumpChipState::Neutral,
        JumpChipState::ResponseReady,
        JumpChipState::ApprovalNeeded,
        JumpChipState::InputNeeded,
    ] {
        let (full_action, compact_action) = state.labels();
        let full = format!("↓ {full_action}  {binding}");
        let compact = format!("↓ {compact_action} {binding}");
        for (rung, width, expected) in [
            ("full", display_width(&full), &full),
            ("compact", display_width(&full) - 1, &compact),
            ("shortcut", display_width(&compact) - 1, &shortcut),
        ] {
            assert_eq!(
                &jump_to_bottom_text(width, binding, false, state),
                expected,
                "{state:?} {rung}"
            );
        }
    }
}

// Covers: every attention compact label is no wider than neutral's, so any
// width that still shows the neutral cue shows the attention cue too.
// Owner: pure unit (chip copy invariant)
#[test]
fn jump_to_bottom_attention_compact_labels_fit_where_neutral_does() {
    let (_, neutral_compact) = JumpChipState::Neutral.labels();
    let neutral_width = display_width(neutral_compact);
    for state in [
        JumpChipState::ResponseReady,
        JumpChipState::ApprovalNeeded,
        JumpChipState::InputNeeded,
    ] {
        let (full, compact) = state.labels();
        assert!(
            display_width(compact) <= neutral_width,
            "{full:?} compact label {compact:?} is wider than neutral's {neutral_compact:?}"
        );
    }
}

fn responding_parent() -> (ActivityPhase, Option<ProviderRetryHint>) {
    (ActivityPhase::Responding, None)
}

fn counts(subagent_count: usize, job_count: usize) -> BackgroundCounts {
    BackgroundCounts {
        subagent_count,
        job_count,
    }
}

// Covers: idle with no background work must not keep an activity rail;
// parent vs background vs linger-only choose distinct variants.
// Owner: pure unit (status construction)
#[test]
fn from_parent_and_background_selects_variant() {
    let parent = Some(responding_parent());
    let (phase, retry) = responding_parent();
    assert_eq!(
        ActivityStatus::from_parent_and_background(None, counts(0, 0), false),
        None
    );
    assert_eq!(
        ActivityStatus::from_parent_and_background(None, counts(0, 0), true),
        Some(ActivityStatus::Linger)
    );
    let cases = [
        (
            parent,
            counts(0, 0),
            ActivityStatus::Parent {
                phase,
                retry,
                background: counts(0, 0),
            },
        ),
        (
            parent,
            counts(2, 0),
            ActivityStatus::Parent {
                phase,
                retry,
                background: counts(2, 0),
            },
        ),
        (
            parent,
            counts(0, 1),
            ActivityStatus::Parent {
                phase,
                retry,
                background: counts(0, 1),
            },
        ),
        (
            parent,
            counts(2, 1),
            ActivityStatus::Parent {
                phase,
                retry,
                background: counts(2, 1),
            },
        ),
        (None, counts(1, 0), ActivityStatus::Background(counts(1, 0))),
        (None, counts(0, 3), ActivityStatus::Background(counts(0, 3))),
        (None, counts(2, 1), ActivityStatus::Background(counts(2, 1))),
    ];
    for (parent, background, expected) in cases {
        assert_eq!(
            ActivityStatus::from_parent_and_background(parent, background, false),
            Some(expected),
        );
    }
}

// Covers: live elapsed trails the widest status label and drops before the
// status ladder degrades.
// Owner: pure unit (activity label assembly)
#[test]
fn activity_label_trails_elapsed_then_drops_it() {
    let elapsed = Some(Duration::from_secs(15));
    for status in [
        ActivityStatus::Parent {
            phase: ActivityPhase::Responding,
            retry: None,
            background: counts(0, 0),
        },
        ActivityStatus::Parent {
            phase: ActivityPhase::Responding,
            retry: None,
            background: counts(2, 0),
        },
        ActivityStatus::Background(counts(2, 0)),
        ActivityStatus::Background(counts(0, 1)),
    ] {
        let widest = activity_status_labels(status).remove(0);
        let timed = activity_label(80, status, elapsed);
        assert!(
            timed.starts_with(&widest) && timed.len() > widest.len(),
            "{status:?}: {timed:?} should trail elapsed after {widest:?}"
        );
        assert_eq!(
            activity_label(display_width(&widest), status, elapsed),
            widest,
            "{status:?}: elapsed drops first"
        );
        assert_eq!(activity_label(80, status, None), widest, "{status:?}");
    }
}

// Covers: every status ladder shrinks strictly rung by rung and bottoms out at
// the bare spinner, so narrow panes always find a fitting label.
// Owner: pure unit (activity label assembly)
#[test]
fn activity_status_labels_shrink_to_bare_spinner() {
    let spinner = LoadingSpinner::FRAMES[0];
    let cases = [
        (
            ActivityStatus::Parent {
                phase: ActivityPhase::RunningTool,
                retry: None,
                background: counts(2, 1),
            },
            4,
        ),
        (
            ActivityStatus::Parent {
                phase: ActivityPhase::RunningTool,
                retry: None,
                background: counts(0, 3),
            },
            4,
        ),
        (ActivityStatus::Background(counts(1, 0)), 4),
        (ActivityStatus::Background(counts(0, 1)), 4),
        (ActivityStatus::Background(counts(2, 1)), 3),
    ];
    for (status, rungs) in cases {
        let labels = activity_status_labels(status);
        assert_eq!(labels.len(), rungs, "{status:?}: {labels:?}");
        assert!(
            labels
                .windows(2)
                .all(|pair| display_width(&pair[1]) < display_width(&pair[0])),
            "{status:?}: {labels:?}"
        );
        assert_eq!(
            labels.last().map(String::as_str),
            Some(spinner),
            "{status:?}"
        );
    }
}

// Covers: rail overflow keeps live rows, then lingering failures, in original order.
// Owner: pure unit (rail row selection)
#[test]
fn select_capped_rail_rows_prioritizes_live_then_failures() {
    #[derive(Debug, PartialEq, Eq)]
    struct Row {
        id: &'static str,
        live: bool,
        fail: bool,
    }
    let rows = [
        Row {
            id: "ok-linger",
            live: false,
            fail: false,
        },
        Row {
            id: "live-a",
            live: true,
            fail: false,
        },
        Row {
            id: "fail-linger",
            live: false,
            fail: true,
        },
        Row {
            id: "live-b",
            live: true,
            fail: false,
        },
    ];
    let (indices, hidden) = select_capped_rail_rows(&rows, 8, |row| row.live, |row| row.fail);
    assert_eq!(indices, [1]);
    assert_eq!(hidden, Some(3));
    assert_eq!(rows[indices[0]].id, "live-a");

    let lingering = [
        Row {
            id: "ok",
            live: false,
            fail: false,
        },
        Row {
            id: "fail",
            live: false,
            fail: true,
        },
        Row {
            id: "ok-2",
            live: false,
            fail: false,
        },
    ];
    let (indices, hidden) = select_capped_rail_rows(&lingering, 8, |row| row.live, |row| row.fail);
    assert_eq!(indices, [1]);
    assert_eq!(hidden, Some(2));
}
