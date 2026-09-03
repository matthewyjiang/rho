use std::time::Duration;

use pretty_assertions::assert_eq;

use super::*;

// Covers: rail rows pack identity · activity  elapsed, drop activity before
// chopping identity, stay within the pane on narrow hover trailing, and use
// the pane width instead of a 52-col clamp.
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
        expected: &'static [&'static str],
    }
    let cases = [
        Case {
            name: "columns pack when they fit",
            identity: &["sleep", " ", "aaaaaaaa"],
            activity: "running",
            trailing: "4s",
            width: 80,
            expected: &[
                "  └ ", "sleep", " ", "aaaaaaaa", "  ·  ", "running", "  ", "4s",
            ],
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
            expected: &[
                "  └ ",
                "◉ ",
                "explorer",
                "  ",
                "TUI Redundancy and Simplification Audit",
                "  ·  ",
                "read",
                "  ",
                "12s",
            ],
        },
        Case {
            name: "narrow row drops activity and keeps elapsed",
            identity: &["very-long-command"],
            activity: "running",
            trailing: "12s",
            width: 18,
            expected: &["  └ ", "very-lon…", "  ", "12s"],
        },
        Case {
            name: "narrow hover trailing stays within the pane",
            identity: &["explorer"],
            activity: "read",
            trailing: "⏎ attach · 4s",
            width: 10,
            expected: &["  └ ", "  ", "⏎ a…"],
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
            &texts[..case.expected.len()],
            case.expected,
            "{}",
            case.name
        );
        assert!(
            texts[case.expected.len()..]
                .iter()
                .all(|text| text.chars().all(|ch| ch == ' ')),
            "{}",
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

// Covers: jump chip copy reflects attention state (response ready / input needed).
// Owner: pure unit (chip copy policy)
#[test]
fn jump_to_bottom_text_reflects_chip_state() {
    let binding = "ctrl+e";
    assert_eq!(
        jump_to_bottom_text(80, binding, false, JumpChipState::Neutral),
        "↓ jump to bottom  ctrl+e"
    );
    assert_eq!(
        jump_to_bottom_text(80, binding, false, JumpChipState::ResponseReady),
        "↓ response ready  ctrl+e"
    );
    assert_eq!(
        jump_to_bottom_text(80, binding, false, JumpChipState::ApprovalNeeded),
        "↓ approval needed  ctrl+e"
    );
    assert_eq!(
        jump_to_bottom_text(80, binding, false, JumpChipState::InputNeeded),
        "↓ input needed  ctrl+e"
    );
}

// Covers: attention states degrade to their compact form before dropping to
// the bare shortcut, so the cue survives narrow terminals.
// Owner: pure unit (chip width degradation)
#[test]
fn jump_to_bottom_attention_states_have_compact_forms() {
    let binding = "ctrl+e";
    // "↓ bottom ctrl+e" is one cell too wide here; neutral falls to shortcut.
    assert_eq!(
        jump_to_bottom_text(14, binding, false, JumpChipState::Neutral),
        "↓ ctrl+e"
    );
    // "↓ ready ctrl+e" fits exactly.
    assert_eq!(
        jump_to_bottom_text(14, binding, false, JumpChipState::ResponseReady),
        "↓ ready ctrl+e"
    );
    assert_eq!(
        jump_to_bottom_text(14, binding, false, JumpChipState::ApprovalNeeded),
        "↓ ask ctrl+e"
    );
    assert_eq!(
        jump_to_bottom_text(14, binding, false, JumpChipState::InputNeeded),
        "↓ input ctrl+e"
    );
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
    let spinner = LoadingSpinner::FRAMES[0];
    let parent = ActivityStatus::Parent {
        phase: ActivityPhase::Responding,
        retry: None,
        background: counts(0, 0),
    };
    let with_agents = ActivityStatus::Parent {
        phase: ActivityPhase::Responding,
        retry: None,
        background: counts(2, 0),
    };
    let agents_only = ActivityStatus::Background(counts(2, 0));
    let jobs_only = ActivityStatus::Background(counts(0, 1));

    let parent_timed = format!("{spinner} responding · 15.0s");
    let parent_plain = format!("{spinner} responding");
    let agents_timed = format!("{spinner} responding  ·  2 agents · 15.0s");
    let agents_plain = format!("{spinner} responding  ·  2 agents");
    let only_timed = format!("{spinner} 2 agents working · 15.0s");
    let jobs_timed = format!("{spinner} 1 job running · 15.0s");
    let elapsed = Some(Duration::from_secs(15));

    assert_eq!(activity_label(80, parent, elapsed), parent_timed);
    assert_eq!(
        activity_label(display_width(&parent_plain), parent, elapsed),
        parent_plain
    );
    assert_eq!(activity_label(80, with_agents, elapsed), agents_timed);
    assert_eq!(
        activity_label(display_width(&agents_plain), with_agents, elapsed),
        agents_plain
    );
    assert_eq!(activity_label(80, agents_only, elapsed), only_timed);
    assert_eq!(activity_label(80, jobs_only, elapsed), jobs_timed);
    assert_eq!(activity_label(80, parent, None), parent_plain);
}

// Covers: mixed background counts compress agents+jobs before dropping to a
// bare spinner, with singular/plural nouns on the wide rungs.
// Owner: pure unit (activity label assembly)
#[test]
fn activity_status_labels_compress_background_counts() {
    let spinner = LoadingSpinner::FRAMES[0];
    let cases = [
        (
            ActivityStatus::Parent {
                phase: ActivityPhase::RunningTool,
                retry: None,
                background: counts(2, 1),
            },
            vec![
                format!("{spinner} running tool  ·  2 agents · 1 job"),
                format!("{spinner} running tool · 2+1"),
                format!("{spinner} 2+1"),
                spinner.into(),
            ],
        ),
        (
            ActivityStatus::Parent {
                phase: ActivityPhase::RunningTool,
                retry: None,
                background: counts(0, 3),
            },
            vec![
                format!("{spinner} running tool  ·  3 jobs"),
                format!("{spinner} running tool · 3"),
                format!("{spinner} 3"),
                spinner.into(),
            ],
        ),
        (
            ActivityStatus::Background(counts(1, 0)),
            vec![
                format!("{spinner} 1 agent working"),
                format!("{spinner} 1 agent"),
                format!("{spinner} 1"),
                spinner.into(),
            ],
        ),
        (
            ActivityStatus::Background(counts(0, 1)),
            vec![
                format!("{spinner} 1 job running"),
                format!("{spinner} 1 job"),
                format!("{spinner} 1"),
                spinner.into(),
            ],
        ),
        (
            ActivityStatus::Background(counts(2, 1)),
            vec![
                format!("{spinner} 2 agents · 1 job"),
                format!("{spinner} 2+1"),
                spinner.into(),
            ],
        ),
    ];
    for (status, expected) in cases {
        assert_eq!(activity_status_labels(status), expected);
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

// Covers: overflow copy is singular for one hidden row and plural otherwise.
// Owner: pure unit (overflow copy)
#[test]
fn overflow_label_singular_and_plural() {
    assert_eq!(overflow_label(1, "agent", "agents"), "1 more agent");
    assert_eq!(overflow_label(2, "job", "jobs"), "2 more jobs");
}
