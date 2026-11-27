use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::methods::{PullRequestId, SessionId};

fn pull_request() -> NotificationSubject {
    NotificationSubject::PullRequest {
        id: PullRequestId::new("github.com", "o", "r", 12),
        url: "https://github.com/o/r/pull/12".into(),
    }
}

// Covers: a card's typed side keeps the wire form clients build chips and
// timeline rows from: snake_case values, each subject tagged by `kind`, each
// item with its own urgency, which may be less than the card's, and a
// subagent's absent session left out; it reads back as written.
// Owner: protocol wire contract.
#[test]
fn notification_info_wire_form() {
    let cases = [
        (
            NotificationInfo::new(
                NotificationId::new("card-1"),
                NotificationUrgency::NeedsYou,
                pull_request(),
                vec![
                    NotificationItem::new(
                        NotificationOutcome::ChecksFailed,
                        NotificationUrgency::NeedsYou,
                        pull_request(),
                        "Checks failed: lint",
                    ),
                    NotificationItem::new(
                        NotificationOutcome::NewComments,
                        NotificationUrgency::Notable,
                        pull_request(),
                        "1 new comment",
                    ),
                ],
            ),
            json!({
                "id": "card-1",
                "urgency": "needs_you",
                "subject": {
                    "kind": "pull_request",
                    "id": {"host": "github.com", "owner": "o", "name": "r", "number": 12},
                    "url": "https://github.com/o/r/pull/12",
                },
                "items": [
                    {
                        "outcome": "checks_failed",
                        "urgency": "needs_you",
                        "subject": {
                            "kind": "pull_request",
                            "id": {"host": "github.com", "owner": "o", "name": "r", "number": 12},
                            "url": "https://github.com/o/r/pull/12",
                        },
                        "text": "Checks failed: lint",
                    },
                    {
                        "outcome": "new_comments",
                        "urgency": "notable",
                        "subject": {
                            "kind": "pull_request",
                            "id": {"host": "github.com", "owner": "o", "name": "r", "number": 12},
                            "url": "https://github.com/o/r/pull/12",
                        },
                        "text": "1 new comment",
                    },
                ],
            }),
        ),
        (
            NotificationInfo::new(
                NotificationId::new("card-2"),
                NotificationUrgency::Routine,
                NotificationSubject::Subagent {
                    run: "run-1".into(),
                    session: None,
                },
                vec![NotificationItem::new(
                    NotificationOutcome::Update,
                    NotificationUrgency::Routine,
                    NotificationSubject::Subagent {
                        run: "run-1".into(),
                        session: None,
                    },
                    "halfway",
                )],
            ),
            json!({
                "id": "card-2",
                "urgency": "routine",
                "subject": {"kind": "subagent", "run": "run-1"},
                "items": [{
                    "outcome": "update",
                    "urgency": "routine",
                    "subject": {"kind": "subagent", "run": "run-1"},
                    "text": "halfway",
                }],
            }),
        ),
        (
            NotificationInfo::new(
                NotificationId::new("card-3"),
                NotificationUrgency::Notable,
                NotificationSubject::Session {
                    id: SessionId::new("s2"),
                },
                vec![NotificationItem::new(
                    NotificationOutcome::Completed,
                    NotificationUrgency::Notable,
                    NotificationSubject::Session {
                        id: SessionId::new("s2"),
                    },
                    "done",
                )],
            ),
            json!({
                "id": "card-3",
                "urgency": "notable",
                "subject": {"kind": "session", "id": "s2"},
                "items": [{
                    "outcome": "completed",
                    "urgency": "notable",
                    "subject": {"kind": "session", "id": "s2"},
                    "text": "done",
                }],
            }),
        ),
    ];
    for (info, wire) in cases {
        assert_eq!(serde_json::to_value(&info).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<NotificationInfo>(wire).unwrap(),
            info
        );
    }
}

// Covers: a card from a newer daemon, with urgencies, an outcome and a
// subject kind this crate does not know, still reads, as `Unknown` each,
// though the unknown subject carries fields of its own, so a client keeps
// the card rather than failing the event. The open-enum table reads bare
// kinds only.
// Owner: protocol wire contract.
#[test]
fn a_newer_daemons_card_reads_with_unknown_values() {
    let wire = json!({
        "id": "card-1",
        "urgency": "on_fire",
        "subject": {"kind": "deployment", "id": "d1"},
        "items": [{
            "outcome": "rolled_back",
            "urgency": "on_fire",
            "subject": {"kind": "deployment", "id": "d1"},
            "text": "rolled back",
        }],
    });
    let expected = NotificationInfo::new(
        NotificationId::new("card-1"),
        NotificationUrgency::Unknown,
        NotificationSubject::Unknown,
        vec![NotificationItem::new(
            NotificationOutcome::Unknown,
            NotificationUrgency::Unknown,
            NotificationSubject::Unknown,
            "rolled back",
        )],
    );
    assert_eq!(
        serde_json::from_value::<NotificationInfo>(wire).unwrap(),
        expected
    );
}
