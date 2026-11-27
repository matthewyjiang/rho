use pretty_assertions::assert_eq;
use rho_protocol::{
    display::{
        NotificationCard, NotificationItem, NotificationOutcome, NotificationSubject,
        NotificationTone, NotificationUrgency,
    },
    AnswerStop, InputRequestId, SessionId,
};

use super::{PeerBatch, PeerEvent, PeerInbox, PeerNotice, PeerTurnEnd};
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

fn answered(stop: AnswerStop) -> PeerTurnEnd {
    PeerTurnEnd::Answered {
        answer: "all green".into(),
        stop,
    }
}

// Covers: one boundary that takes notices of two sessions renders them
// once, in the order they arrived: one model input naming both sessions,
// and one transcript card per notice. A miss drops a session's news or
// shows it twice.
// Owner: started-session notices at the boundary.
#[test]
fn a_boundary_renders_one_card_per_notice_in_order() {
    let inbox = PeerInbox::default();
    inbox
        .expect_for_tests("child-a")
        .tell_ended(Some("Fix the build".into()), answered(AnswerStop::EndTurn));
    let child_b = inbox.expect_for_tests("child-b");
    child_b.tell_waiting(
        /*title*/ None,
        InputRequestId(3),
        "Pick a database".into(),
    );
    let batch = inbox.take();
    let cards: Vec<NotificationCard> = batch.rows().into_iter().map(|row| row.card).collect();

    let (model, transcript) = rendered(&batch).expect("the notices render");
    let named = |session: &str| model.find(session).expect("the session is named");
    let senders: Vec<String> = cards.iter().map(|card| card.sender.clone()).collect();
    let shown: Vec<NotificationCard> = transcript
        .0
        .into_iter()
        .map(|row| match row {
            DisplayRow::Notification(row) => row.card,
            other => panic!("a notice renders as a card, not {other:?}"),
        })
        .collect();

    assert_eq!(
        (named("child-a") < named("child-b"), senders, shown),
        (
            true,
            vec!["child-a".to_owned(), "child-b".to_owned()],
            cards
        )
    );
}

// Covers: an answer the model cut off or refused does not read as a
// success, and a question needs the reader while a finished answer is only
// notable. A miss tells the starter that a refused turn did its work, or
// files a question among routine cards.
// Owner: started-session notices.
#[test]
fn only_a_finished_answer_reads_as_success() {
    use NotificationOutcome as Outcome;
    use NotificationUrgency as Urgency;
    let ask = || PeerEvent::WaitingForInput {
        request: InputRequestId(3),
        ask: "Pick a database".into(),
    };
    let ended = |stop| PeerEvent::TurnEnded(answered(stop));
    let cases = [
        (
            ended(AnswerStop::EndTurn),
            NotificationTone::Success,
            Outcome::Completed,
            Urgency::Notable,
        ),
        (
            ended(AnswerStop::MaxTokens),
            NotificationTone::Warning,
            Outcome::CutOff,
            Urgency::Notable,
        ),
        (
            ended(AnswerStop::Refusal),
            NotificationTone::Warning,
            Outcome::Refused,
            Urgency::Notable,
        ),
        (
            ask(),
            NotificationTone::Warning,
            Outcome::Question,
            Urgency::NeedsYou,
        ),
    ];
    for (event, tone, outcome, urgency) in cases {
        let name = format!("{event:?}");
        let notice = PeerNotice {
            session: "child".into(),
            title: None,
            event,
        };
        let row = notice.row();
        let info = row.info.expect("a notice is typed");
        let session = NotificationSubject::Session {
            id: SessionId::new("child"),
        };
        assert_eq!(
            (row.card.tone, info.urgency, &info.subject, &info.items[..]),
            (
                tone,
                urgency,
                &session,
                &[NotificationItem::new(
                    outcome,
                    urgency,
                    tone,
                    session.clone(),
                    row.card.title.clone()
                )][..]
            ),
            "{name}"
        );
    }
}

/// What one step does to the inbox.
#[derive(Debug)]
enum Step {
    /// Session `0` asks question `1`.
    Ask(&'static str, u64),
    /// Session `0`'s turn ends.
    End(&'static str),
    /// A boundary takes the notices; the next [`Step::Restore`] puts them
    /// back.
    Take,
    Restore,
}

fn asking(session: &str, request: u64) -> PeerNotice {
    PeerNotice {
        session: session.into(),
        title: None,
        event: PeerEvent::WaitingForInput {
            request: InputRequestId(request),
            ask: "input".into(),
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
// session first, so a session that asked and then ended tells only of its
// end; a boundary that puts back what it took never hides a newer notice
// of the same session. A miss tells the starter twice, or tells it stale
// news.
// Owner: started-session notices.
#[test]
fn the_inbox_keeps_each_sessions_latest_notice() {
    let cases: [(&str, Vec<Step>, Vec<PeerNotice>); 3] = [
        (
            "an end replaces the question",
            vec![Step::Ask("a", 0), Step::End("b"), Step::End("a")],
            vec![ended("b"), ended("a")],
        ),
        (
            "a question that came after the take",
            vec![
                Step::Ask("a", 0),
                Step::Take,
                Step::Ask("a", 1),
                Step::Restore,
            ],
            vec![asking("a", 1)],
        ),
        (
            "a take put back goes first",
            vec![
                Step::Ask("a", 0),
                Step::Take,
                Step::Ask("b", 1),
                Step::Restore,
            ],
            vec![asking("a", 0), asking("b", 1)],
        ),
    ];
    for (name, steps, expected) in cases {
        let inbox = PeerInbox::default();
        let mut a = Some(inbox.expect_for_tests("a"));
        let mut b = Some(inbox.expect_for_tests("b"));
        let mut taken = PeerBatch::default();
        for step in steps {
            match step {
                Step::Ask(session, request) => {
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
        assert_eq!(inbox.take().notices, expected, "{name}");
    }
}

// Covers: a turn owes a notice once its prompt's registration is claimed,
// until it ends or its expectation goes unsent, as when the started session
// goes first; a later turn's expectation outlives the earlier one's; a
// registration that goes unclaimed, as for a refused prompt, owes nothing
// and cannot be claimed after. A leak keeps the starter loaded and active
// forever.
// Owner: started-session notices.
#[test]
fn a_turn_owes_a_notice_until_it_ends_or_goes() {
    let inbox = PeerInbox::default();
    let expecting = || (inbox.expecting(), inbox.outstanding());

    let refused = inbox.register("a");
    let key = refused.key();
    let while_registered = expecting();
    drop(refused);
    let claimed_after_refusal = inbox.claim("a", &key).is_some();
    let owed = inbox.expect_for_tests("a");
    let while_owed = expecting();
    owed.tell_ended(/*title*/ None, PeerTurnEnd::Interrupted);
    let once_told = expecting();
    inbox.take();
    let once_taken = expecting();
    let earlier = inbox.expect_for_tests("a");
    let later = inbox.expect_for_tests("a");
    drop(earlier);
    let with_a_later_turn = expecting();
    drop(later);
    let once_gone = expecting();

    assert_eq!(
        (
            while_registered,
            claimed_after_refusal,
            [
                while_owed,
                once_told,
                once_taken,
                with_a_later_turn,
                once_gone
            ]
        ),
        (
            (false, false),
            false,
            [
                (true, true),
                (false, true),
                (false, false),
                (true, true),
                (false, false),
            ]
        )
    );
}
