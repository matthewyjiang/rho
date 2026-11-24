use pretty_assertions::assert_eq;
use rho_protocol::{
    display::{
        NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
        NotificationVisibility,
    },
    InputRequestId,
};

use super::{PeerBatch, PeerEvent, PeerInbox, PeerNotice, PeerTurnEnd, MODEL_NOTIFICATION_BYTES};
use crate::{
    app::{boundary_results::BoundaryResults, pull_requests::NewsBatch},
    display_transcript::{DisplayRow, DisplayTranscript},
};

/// The model input and transcript rows a boundary that took `batch`, and
/// nothing else, renders.
fn rendered(batch: &PeerBatch) -> Option<(String, DisplayTranscript)> {
    BoundaryResults {
        notices: &[],
        subagents: &[],
        workflows: &[],
        processes: &[],
        surface_actions: &[],
        pull_requests: &NewsBatch::default(),
        peer_sessions: batch,
    }
    .render(/*manager*/ None)
    .finish()
}

/// A card for a notice of session `session`, as every one of them is
/// addressed.
fn card(
    session: &str,
    title: &str,
    tone: NotificationTone,
    preview: NotificationPreview,
    body: &str,
) -> NotificationCard {
    NotificationCard {
        title: title.into(),
        sender: session.into(),
        recipient: "parent".into(),
        delivery: NotificationDelivery::Received,
        tone,
        preview,
        visibility: NotificationVisibility::Conversation,
        reference: Some(session.into()),
        subtitle: None,
        body: body.into(),
        output: None,
        details: vec![format!("session: {session}")],
    }
}

// Covers: a notice that a started session's turn ended, and one that it
// waits for input, render once for the model and the transcript: the
// model reads the session, its title, its state and outcome, the final
// answer, or the input request and that `sessions wait` returns it, and
// where to read more; the transcript shows one card per session, toned as
// a subagent's result card is.
// Owner: started-session notices at the boundary.
#[test]
fn started_session_notices_render_for_the_model_and_the_transcript() {
    let inbox = PeerInbox::new();
    let footer = "\n\nRead a session's whole transcript with `sessions read`. rho tells you when a turn you started in a session ends or waits for a person, so there is no need to poll with `sessions wait`; if a `sessions wait` already returned this, nothing is new.\n\nThis is an automated notification, not a user message.";
    let ended = || {
        inbox.expect("child-a").tell_ended(
            Some("Fix the build".into()),
            PeerTurnEnd::Answered {
                answer: "all green".into(),
            },
        );
    };
    let waiting = || {
        inbox.expect("child-b").tell_waiting(
            /*title*/ None,
            InputRequestId(3),
            "Pick a database".into(),
        );
    };
    let cases: [(&str, &dyn Fn(), String, NotificationCard); 2] = [
        (
            "ended",
            &ended,
            format!(
                "[session notification]\n\nSessions you started:\nsession child-a (Fix the build): completed\noutcome: answered\n\nFinal answers and errors:\n\nsession child-a:\nall green{footer}"
            ),
            card(
                "child-a",
                "Completed · Fix the build",
                NotificationTone::Success,
                NotificationPreview::Truncated,
                "all green",
            ),
        ),
        (
            "waiting",
            &waiting,
            format!(
                "[session notification]\n\nSessions you started:\nsession child-b: waiting_for_input\nwaits for a person to answer input request 3: Pick a database. `sessions wait` returns the whole question.{footer}"
            ),
            card(
                "child-b",
                "Waiting for input · child-b",
                NotificationTone::Warning,
                NotificationPreview::Full,
                "input request 3: Pick a database",
            ),
        ),
    ];
    for (name, tell, model, card) in cases {
        tell();
        assert_eq!(
            rendered(&inbox.take()),
            Some((
                model,
                DisplayTranscript(vec![DisplayRow::Notification(Box::new(card))])
            )),
            "{name}"
        );
    }
}

// Covers: answers too long for one notification are cut so the whole text
// fits the subagent notification budget, and every session's state still
// shows, ahead of the answers. Otherwise one long answer hides how the
// other sessions ended, or the wake overflows its budget.
// Owner: started-session notices at the boundary.
#[test]
fn long_answers_are_cut_to_the_notification_budget() {
    let inbox = PeerInbox::new();
    let sessions = ["child-a", "child-b", "child-c"];
    for session in sessions {
        inbox.expect(session).tell_ended(
            /*title*/ None,
            PeerTurnEnd::Answered {
                answer: "x".repeat(MODEL_NOTIFICATION_BYTES),
            },
        );
    }
    let model = inbox.take().prompt();
    let states: Vec<&str> = model
        .lines()
        .filter(|line| line.ends_with(": completed"))
        .collect();

    assert_eq!(
        (model.len() <= MODEL_NOTIFICATION_BYTES, states),
        (
            true,
            vec![
                "session child-a: completed",
                "session child-b: completed",
                "session child-c: completed",
            ]
        ),
        "{}-byte text over the {MODEL_NOTIFICATION_BYTES}-byte budget",
        model.len()
    );
}

/// What one step does to the inbox.
#[derive(Debug)]
enum Step {
    /// Session `0` waits for input request `1`.
    Wait(&'static str, u64),
    /// Session `0`'s turn ends.
    End(&'static str),
    /// A boundary takes the notices; the next [`Step::Restore`] puts them
    /// back.
    Take,
    Restore,
}

fn waiting(session: &str, request: u64) -> PeerNotice {
    PeerNotice {
        session: session.into(),
        title: None,
        event: PeerEvent::WaitingForInput {
            request: InputRequestId(request),
            title: "input".into(),
        },
    }
}

fn ended(session: &str) -> PeerNotice {
    PeerNotice {
        session: session.into(),
        title: None,
        event: PeerEvent::TurnEnded(PeerTurnEnd::Cancelled),
    }
}

// Covers: the inbox keeps each started session's latest notice, oldest
// session first, so a session that waited and then ended tells only of its
// end; a boundary that puts back what it took never hides a newer notice
// of the same session; and a turn owes a notice until it ends or its
// expectation goes. A miss tells the starter twice, tells it stale news,
// or keeps it loaded forever.
// Owner: started-session notices.
#[test]
fn the_inbox_keeps_each_sessions_latest_notice() {
    let cases: [(&str, Vec<Step>, Vec<PeerNotice>); 3] = [
        (
            "an end replaces the wait",
            vec![Step::Wait("a", 0), Step::End("b"), Step::End("a")],
            vec![ended("b"), ended("a")],
        ),
        (
            "a wait that came after the take",
            vec![
                Step::Wait("a", 0),
                Step::Take,
                Step::Wait("a", 1),
                Step::Restore,
            ],
            vec![waiting("a", 1)],
        ),
        (
            "a take put back",
            vec![
                Step::Wait("a", 0),
                Step::Wait("b", 1),
                Step::Take,
                Step::Restore,
            ],
            vec![waiting("a", 0), waiting("b", 1)],
        ),
    ];
    for (name, steps, expected) in cases {
        let inbox = PeerInbox::new();
        let (a, b) = (inbox.expect("a"), inbox.expect("b"));
        let (mut a, mut b) = (Some(a), Some(b));
        let mut taken = PeerBatch::default();
        for step in steps {
            match step {
                Step::Wait(session, request) => {
                    let owed = if session == "a" { &a } else { &b };
                    owed.as_ref().unwrap().tell_waiting(
                        /*title*/ None,
                        InputRequestId(request),
                        "input".into(),
                    );
                }
                Step::End(session) => {
                    let owed = if session == "a" { &mut a } else { &mut b };
                    owed.take()
                        .unwrap()
                        .tell_ended(/*title*/ None, PeerTurnEnd::Cancelled);
                }
                Step::Take => taken = inbox.take(),
                Step::Restore => inbox.restore(&taken),
            }
        }
        assert_eq!(inbox.take().notices_for_tests(), expected, "{name}");
    }
}

// Covers: a started turn owes a notice from its expectation until it ends,
// or until the expectation goes unsent, as when the started session goes
// first; a later turn's expectation outlives the earlier one's. A leak
// keeps the starter loaded and active forever.
// Owner: started-session notices.
#[test]
fn a_turn_owes_a_notice_until_it_ends_or_goes() {
    let inbox = PeerInbox::new();
    let expecting = || (inbox.expecting(), inbox.outstanding());

    let owed = inbox.expect("a");
    let while_owed = expecting();
    owed.tell_ended(/*title*/ None, PeerTurnEnd::Interrupted);
    let once_told = expecting();
    inbox.take();
    let once_taken = expecting();
    let earlier = inbox.expect("a");
    let later = inbox.expect("a");
    drop(earlier);
    let with_a_later_turn = expecting();
    drop(later);
    let once_gone = expecting();

    assert_eq!(
        [
            while_owed,
            once_told,
            once_taken,
            with_a_later_turn,
            once_gone
        ],
        [
            (true, true),
            (false, true),
            (false, false),
            (true, true),
            (false, false),
        ]
    );
}
