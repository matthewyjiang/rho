use pretty_assertions::assert_eq;

use super::{
    candidate_agent_id, candidate_item, merge_live_candidates, picker, retire_departed_live_runs,
    visible_candidates, AttachCandidate, WorkspaceRunFilter,
};
use crate::subagent::RunState;

// Covers: attach rows show role, title, and current activity instead of a run id.
// Owner: attach picker
#[test]
fn attach_row_uses_role_title_and_activity() {
    let cases = [
        (
            AttachCandidate {
                run_id: "abc123".into(),
                agent_id: "worker".into(),
                title: Some("Review the auth path".into()),
                last_activity: Some("tool: read".into()),
                state: RunState::Running,
                elapsed_seconds: 12,
            },
            "worker",
            "Review the auth path",
            "read",
        ),
        (
            AttachCandidate {
                run_id: "def456".into(),
                agent_id: "explorer".into(),
                title: None,
                last_activity: None,
                state: RunState::Starting,
                elapsed_seconds: 1,
            },
            "explorer",
            "untitled",
            "starting",
        ),
    ];

    for (candidate, role, label, activity) in cases {
        let run_id = candidate.run_id.clone();
        let item = candidate_item(&candidate);
        assert_eq!(item.section.as_deref(), Some(role));
        assert_eq!(item.label, label);
        assert_eq!(item.value, run_id);
        assert_eq!(
            item.badge.as_ref().map(|badge| badge.text.as_str()),
            Some(activity)
        );
        assert!(!item.detail.as_deref().unwrap_or_default().contains(&run_id));
    }
}

fn candidate(run_id: &str, state: RunState) -> AttachCandidate {
    AttachCandidate {
        run_id: run_id.into(),
        agent_id: "worker".into(),
        title: Some(run_id.into()),
        last_activity: None,
        state,
        elapsed_seconds: 1,
    }
}

// Covers: running-only must hide finished transcripts until the user toggles.
// Owner: attach picker
#[test]
fn running_only_hides_terminal_runs() {
    let candidates = [
        candidate("aaaaaa", RunState::Running),
        candidate("bbbbbb", RunState::Ok),
        candidate("cccccc", RunState::Error),
    ];

    let running_ids = visible_candidates(&candidates, WorkspaceRunFilter::RunningOnly)
        .into_iter()
        .map(|run| run.run_id.as_str())
        .collect::<Vec<_>>();
    let all_ids = visible_candidates(&candidates, WorkspaceRunFilter::All)
        .into_iter()
        .map(|run| run.run_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(running_ids, ["aaaaaa"]);
    assert_eq!(all_ids, ["aaaaaa", "bbbbbb", "cccccc"]);
}

// Covers: an empty inventory must still build an attach overlay.
// Owner: attach picker
#[test]
fn empty_inventory_still_builds_a_picker() {
    let empty = picker(&[], WorkspaceRunFilter::RunningOnly);
    assert!(empty.items.is_empty());
    assert_eq!(empty.selected_item().map(|item| item.value.as_str()), None);
}

// Covers: live panel rows overlay matching disk rows and keep panel order for new lives.
// Owner: attach picker
#[test]
fn live_candidates_replace_matching_disk_rows() {
    let disk = vec![
        candidate("aaaaaa", RunState::Ok),
        candidate("bbbbbb", RunState::Running),
    ];
    let mut live = candidate("bbbbbb", RunState::Running);
    live.title = Some("updated".into());
    live.elapsed_seconds = 9;
    let first_missing = candidate("cccccc", RunState::Starting);
    let second_missing = candidate("dddddd", RunState::Starting);

    let merged = merge_live_candidates(disk, vec![live, first_missing, second_missing]);

    assert_eq!(
        merged
            .iter()
            .map(|run| (
                run.run_id.as_str(),
                run.title.as_deref(),
                run.elapsed_seconds
            ))
            .collect::<Vec<_>>(),
        [
            ("cccccc", Some("cccccc"), 1),
            ("dddddd", Some("dddddd"), 1),
            ("aaaaaa", Some("aaaaaa"), 1),
            ("bbbbbb", Some("updated"), 9),
        ]
    );
}

// Covers: finished transcripts keep their role after the picker closes.
// Owner: attach picker
#[test]
fn finished_run_agent_id_comes_from_candidates() {
    let mut finished = candidate("aaaaaa", RunState::Ok);
    finished.agent_id = "explorer".into();

    assert_eq!(candidate_agent_id(&[finished], "aaaaaa"), Some("explorer"));
    assert_eq!(candidate_agent_id(&[], "aaaaaa"), None);
}

// Covers: a run that leaves the live panel must not stay listed as running.
// Owner: attach picker
#[test]
fn departed_live_run_is_no_longer_running() {
    let mut candidates = vec![
        candidate("aaaaaa", RunState::Running),
        candidate("bbbbbb", RunState::Running),
    ];
    let previously_live = ["aaaaaa".into(), "bbbbbb".into()].into();
    let live_ids = ["bbbbbb".into()].into();

    retire_departed_live_runs(&mut candidates, &live_ids, &previously_live);

    assert_eq!(
        candidates
            .iter()
            .map(|run| (run.run_id.as_str(), run.state))
            .collect::<Vec<_>>(),
        [("aaaaaa", RunState::Stopped), ("bbbbbb", RunState::Running)]
    );
}
