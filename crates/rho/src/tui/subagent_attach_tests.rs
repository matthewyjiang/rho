use pretty_assertions::assert_eq;

use super::{next_target, parent_notice, ParentWait};
use crate::tui::attachment::ParentNotice;
use crate::tui::subagent_panel::SubagentAttachTarget;

fn target(run_id: &str) -> SubagentAttachTarget {
    SubagentAttachTarget {
        run_id: run_id.to_string(),
        agent_id: format!("agent-{run_id}"),
    }
}

// Covers: Tab must wrap, no-op on a singleton, and never jump when current is
// not in the live rail (a finished /attach target).
// Owner: attach cycle policy
#[test]
fn next_target_only_cycles_the_live_set() {
    let two = [target("aa0001"), target("aa0002")];
    let one = [target("aa0001")];
    let cases = [
        ("aa0001", two.as_slice(), 1, Some("aa0002")),
        ("aa0002", two.as_slice(), 1, Some("aa0001")),
        ("aa0001", two.as_slice(), -1, Some("aa0002")),
        ("aa0001", one.as_slice(), 1, None),
        ("dead01", two.as_slice(), 1, None),
        ("aa0001", [].as_slice(), 1, None),
    ];
    for (current, targets, delta, expected) in cases {
        assert_eq!(
            next_target(current, targets, delta).map(|target| target.run_id.as_str()),
            expected,
            "current={current} delta={delta}"
        );
    }
}

// Covers: parent approval/questionnaire win; turn-complete stays armed across
// a cycle only when it was armed and the parent is no longer busy.
// Owner: attach parent-notice policy
#[test]
fn parent_notice_prefers_composer_wait_then_armed_turn() {
    let cases = [
        (
            Some(ParentWait::Approval),
            true,
            false,
            Some(ParentNotice::Approval),
        ),
        (
            Some(ParentWait::Approval),
            true,
            true,
            Some(ParentNotice::Approval),
        ),
        (
            Some(ParentWait::Questionnaire),
            true,
            false,
            Some(ParentNotice::Questionnaire),
        ),
        (None, true, false, Some(ParentNotice::TurnComplete)),
        (None, true, true, None),
        (None, false, false, None),
    ];
    for (waiting, armed, busy, expected) in cases {
        assert_eq!(
            parent_notice(waiting, armed, busy),
            expected,
            "waiting={waiting:?} armed={armed} busy={busy}"
        );
    }
}
