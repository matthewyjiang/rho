use std::time::Duration;

use super::*;

// Covers: stacked rail rows share one column layout for identity / activity / elapsed.
// Owner: pure layout
#[test]
fn rail_row_columns_identity_activity_and_trailing() {
    let row_style = Theme::activity_rail();
    let line = RailRow {
        connector: tree_connector(true),
        identity: vec![
            Span::styled("sleep", Theme::text_strong().patch(row_style)),
            Span::styled(" ", row_style),
            Span::styled("aaaaaaaa", Theme::dim().patch(row_style)),
        ],
        activity: "running".into(),
        trailing: "4s".into(),
        row_style,
    }
    .into_line(80);

    let texts: Vec<&str> = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(texts[0], "  └ ");
    assert_eq!(texts[1], "sleep");
    assert_eq!(texts[2], " ");
    assert_eq!(texts[3], "aaaaaaaa");
    assert_eq!(texts[4], "  ·  ");
    assert_eq!(texts[5], "running");
    assert!(texts[6].chars().all(|ch| ch == ' '));
    assert_eq!(texts[7], "4s");
}

// Covers: a too-narrow rail collapses to a single truncated detail span.
// Owner: pure layout
#[test]
fn rail_row_truncates_when_fixed_width_overflows() {
    let row_style = Theme::activity_rail();
    let line = RailRow {
        connector: tree_connector(true),
        identity: vec![Span::styled(
            "very-long-command",
            Theme::text_strong().patch(row_style),
        )],
        activity: "running".into(),
        trailing: "12s".into(),
        row_style,
    }
    .into_line(18);

    assert_eq!(line.spans.len(), 2);
    assert_eq!(line.spans[0].content.as_ref(), "  └ ");
    let detail = line.spans[1].content.as_ref();
    assert!(detail.starts_with("very-long-"));
    assert!(display_width(detail) <= 14);
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

// Covers: live elapsed trails the widest status label and drops before the
// status ladder degrades.
// Owner: pure unit (activity label assembly)
#[test]
fn activity_label_trails_elapsed_then_drops_it() {
    let spinner = LoadingSpinner::FRAMES[0];
    let parent = ActivityStatus::Parent {
        phase: ActivityPhase::Responding,
        retry: None,
    };
    let with_agents = ActivityStatus::ParentWithSubagents {
        phase: ActivityPhase::Responding,
        retry: None,
        subagent_count: 2,
    };
    let agents_only = ActivityStatus::Subagents(2);

    let parent_timed = format!("{spinner} responding · 15.0s");
    let parent_plain = format!("{spinner} responding");
    let agents_timed = format!("{spinner} responding  ·  2 agents · 15.0s");
    let agents_plain = format!("{spinner} responding  ·  2 agents");
    let only_timed = format!("{spinner} 2 agents working · 15.0s");
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
    assert_eq!(activity_label(80, parent, None), parent_plain);
}
