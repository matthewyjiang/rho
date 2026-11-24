//! Tells a session of the turns it started in other sessions through
//! session control (the `sessions` tool's `new` and `prompt`): such a turn
//! that ends, or stops to wait for a person's input, leaves a notice in
//! the starter's `PeerInbox` (see `app::peer_sessions`). The starter's turn under way takes it at
//! its next boundary; an idle starter is woken by a turn the daemon starts
//! for it (see `idle_wake`), as for its finished background work, with no
//! client attached. The inbox keeps the latest notice of each started
//! session, so one delivery turn takes them all.
//!
//! Only a starter whose client asked the daemon to start its idle turns
//! (`idle_wake: turn`) has an inbox; no other would be woken to take a
//! notice. Its started turns still under way keep it loaded and active
//! (see `unload` and `settle`), so the notice finds it.
//!
//! A started session's turn owes a notice from its start, which the
//! prompt that started it records (see [`starter_to_tell`]), until it ends;
//! every turn start forgets what the last one owed, so a turn the daemon
//! starts in the started session owes none. Nothing is saved: a daemon
//! restart forgets the notices not yet delivered and the turns that owed
//! one.
//!
//! The starter is told even while its turn waits on the same session with
//! `sessions wait`: that wait runs on a connection of its own, which the
//! daemon cannot tell from another caller's. The notice says a wait may
//! already have returned it.

use std::sync::{MutexGuard, PoisonError};

use rho_protocol::{self as wire, SessionGrant, TurnId};

use super::{
    super::jobs::{JobId, TurnEnd},
    LoadedSession, Sessions,
};
use crate::app::peer_sessions::{Expectation, PeerTurnEnd};

/// The job whose session a prompt's turn must tell of its end and of its
/// waits for input: the job that started the prompted session, `parent`,
/// when the prompt named its grant.
///
/// Only session control names a prompt's grant (`app::session_control`'s
/// `new` and `prompt`); every other client of the daemon leaves it to its
/// environment. So the starter hears of the turns it began, and not of
/// one a person began in a session it handed to them. The rule is
/// imprecise: a prompt carries no caller, so a turn another session-control
/// caller begins in that session tells the starter too.
fn starter_to_tell<'a>(
    grant: Option<&SessionGrant>,
    parent: Option<&'a JobId>,
) -> Option<&'a JobId> {
    grant.and(parent)
}

/// A started session's turn that owes its starter a notice.
pub(super) struct StarterTurn {
    turn: TurnId,
    expectation: Expectation,
}

impl Sessions {
    /// Records that `session`'s turn `turn`, which a prompt naming `grant`
    /// just started, owes the session that started `session` a notice, if
    /// [`starter_to_tell`] says so and that session is loaded with an
    /// inbox. Call under `session`'s turn lock, right after the turn
    /// started.
    pub(super) fn expect_starter_notice(
        &self,
        session: &LoadedSession,
        turn: &TurnId,
        grant: Option<&SessionGrant>,
    ) {
        let parent = self.jobs.get(&session.job).and_then(|job| job.parent);
        let Some(starter) = starter_to_tell(grant, parent.as_ref()) else {
            return;
        };
        // The turn lock comes before `loaded`, never after.
        let inbox = self
            .loaded()
            .values()
            .find(|loaded| loaded.job == *starter)
            .and_then(|loaded| loaded.peers.clone());
        let Some(inbox) = inbox else {
            return;
        };
        *session.starter_turn() = Some(StarterTurn {
            turn: turn.clone(),
            expectation: inbox.expect(session.id.as_str()),
        });
    }
}

impl LoadedSession {
    fn starter_turn(&self) -> MutexGuard<'_, Option<StarterTurn>> {
        // Only ever replaced or taken whole.
        self.starter_turn
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// A turn starts, which owes no notice unless the prompt that started
    /// it records one after. Call under the turn lock.
    pub(super) fn forget_starter_turn(&self) {
        *self.starter_turn() = None;
    }

    /// Tells the starter, if turn `turn` owes it a notice, that the turn
    /// ended as `end` with `outcome`.
    pub(super) fn tell_starter_ended(
        &self,
        turn: &TurnId,
        outcome: Option<&wire::TurnOutcome>,
        end: &TurnEnd,
    ) {
        let owed = {
            let mut slot = self.starter_turn();
            match slot.as_ref() {
                Some(owed) if owed.turn == *turn => slot.take(),
                Some(_) | None => None,
            }
        };
        if let Some(StarterTurn { expectation, .. }) = owed {
            expectation.tell_ended(self.feed.title(), turn_end(end, outcome));
        }
    }

    /// Tells the starter, if the turn under way owes it a notice, that the
    /// session waits for each of the input requests `opened` (see
    /// [`opened_inputs`]).
    pub(super) fn tell_starter_waiting(&self, opened: Vec<OpenedInput>) {
        if opened.is_empty() {
            return;
        }
        let title = self.feed.title();
        let slot = self.starter_turn();
        let Some(owed) = slot.as_ref() else {
            return;
        };
        for OpenedInput { request, ask } in opened {
            owed.expectation.tell_waiting(title.clone(), request, ask);
        }
    }
}

/// An input request that opened, and what it asks in a few words.
pub(super) struct OpenedInput {
    request: wire::InputRequestId,
    ask: String,
}

/// The input requests `events`, those of one change of the session's
/// input requests, open. Each request opens once.
pub(super) fn opened_inputs(events: &[wire::SessionEvent]) -> Vec<OpenedInput> {
    events
        .iter()
        .filter_map(|event| match event {
            wire::SessionEvent::InputRequested { request } => Some(OpenedInput {
                request: request.id,
                ask: ask_title(&request.ask),
            }),
            _ => None,
        })
        .collect()
}

/// How a turn that ended as `end` with `outcome` reads to its starter.
fn turn_end(end: &TurnEnd, outcome: Option<&wire::TurnOutcome>) -> PeerTurnEnd {
    match end {
        TurnEnd::Completed => match outcome {
            Some(wire::TurnOutcome::Answered { text, .. }) => PeerTurnEnd::Answered {
                answer: text.clone(),
            },
            Some(wire::TurnOutcome::MaxSteps { text }) => PeerTurnEnd::StepLimit {
                reply: text.clone(),
            },
            // A completed turn records one of those two.
            _ => PeerTurnEnd::Answered {
                answer: String::new(),
            },
        },
        TurnEnd::Failed(error) => PeerTurnEnd::Failed {
            error: error.clone(),
        },
        TurnEnd::Cancelled => PeerTurnEnd::Cancelled,
        TurnEnd::TimedOut { timeout_ms } => PeerTurnEnd::TimedOut {
            timeout_ms: *timeout_ms,
        },
        TurnEnd::Interrupted => PeerTurnEnd::Interrupted,
    }
}

/// What an input request asks, in a few words: a question's title, or the
/// kind of approval.
fn ask_title(ask: &wire::Ask) -> String {
    match ask {
        wire::Ask::Question(question) if !question.title.trim().is_empty() => {
            question.title.clone()
        }
        wire::Ask::Question(_) => "questions".into(),
        wire::Ask::Approval(_) => "tool approval".into(),
        wire::Ask::PlanApproval(_) => "plan approval".into(),
        _ => "input".into(),
    }
}

#[cfg(test)]
#[path = "peer_wake_tests.rs"]
mod tests;
