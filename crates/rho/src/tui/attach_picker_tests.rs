use pretty_assertions::assert_eq;

use super::{candidate_item, AttachCandidate};
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
