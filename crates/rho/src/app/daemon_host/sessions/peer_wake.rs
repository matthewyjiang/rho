//! Tells a session of the turns it starts in other sessions through session
//! control (the `sessions` tool's `new` and `prompt`): such a turn that
//! ends, or asks a question, leaves a notice in the starter's `PeerInbox`
//! (see `app::peer_sessions`, which says what the inbox keeps and forgets).
//! The starter's turn under way takes it at its next boundary; an idle
//! starter is woken by a turn the daemon starts for it (see `idle_wake`), as
//! for its finished background work, with no client attached.
//!
//! Only a session opened with `idle_wake: turn` has an inbox: no other would
//! be woken to take a notice. Its `sessions` tool registers each prompt
//! before it sends it, under the idempotency key the prompt carries, and a
//! turn owes a notice only when its prompt's key claims a registration as
//! the turn starts (see [`Sessions::claim_starter`]). So a turn tells the
//! session whose call started it, and nobody else: a turn a person, another
//! caller, or the daemon starts tells nobody, and neither does one a caller
//! without an inbox starts. Every turn start forgets what the last turn
//! owed. A prompt whose turn ends before it began reports that end itself,
//! and tells nobody. A turn that owes a notice keeps its starter loaded and
//! active (see `unload` and `settle`), so the notice finds it.
//!
//! Of what a turn waits for, only questions are told: only a person can
//! answer an approval, and the person's client shows it. A question that
//! closes before a boundary took its notice withdraws it.
//!
//! The starter is told even while its turn waits on the same session with
//! `sessions wait`: that wait runs on a connection of its own, which the
//! daemon cannot tell from another caller's.

use std::sync::{Mutex, MutexGuard, PoisonError};

use rho_protocol::{self as wire, IdempotencyKey};

use super::{super::jobs::TurnEnd, LoadedSession, Sessions};
use crate::app::{
    peer_sessions::{Expectation, PeerInbox, PeerTurnEnd},
    session_runtime::SessionRuntime,
};

/// A loaded session's part in telling sessions of the turns they started.
pub(super) struct PeerWake {
    /// Where notices of the turns this session starts arrive, when its
    /// client asked for daemon turns.
    inbox: Option<PeerInbox>,
    /// What the turn under way owes the session whose call started it.
    owed: Mutex<Option<Expectation>>,
}

impl PeerWake {
    pub(super) fn of(runtime: &SessionRuntime) -> Self {
        Self {
            inbox: runtime.peer_sessions().cloned(),
            owed: Mutex::new(None),
        }
    }

    pub(super) fn inbox(&self) -> Option<&PeerInbox> {
        self.inbox.as_ref()
    }

    fn owed(&self) -> MutexGuard<'_, Option<Expectation>> {
        // Only ever replaced or taken whole.
        self.owed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The turn under way owes nothing: it just started, and its prompt
    /// claims what it owes after, or it ended before it began. Call under
    /// the turn lock.
    pub(super) fn forget(&self) {
        *self.owed() = None;
    }

    /// What the turn that ended as `end` with `outcome` tells the session
    /// that started it, if it owes it a notice; tell it once the job
    /// recorded the end. Call under the turn lock.
    pub(super) fn ended(
        &self,
        end: &TurnEnd,
        outcome: Option<&wire::TurnOutcome>,
    ) -> Option<(Expectation, PeerTurnEnd)> {
        let owed = self.owed().take()?;
        Some((owed, turn_end(end, outcome)))
    }

    /// Tells the session that started the turn under way, if the turn owes
    /// it a notice, of the questions `changes` asked and closed, with the
    /// title `title` gives.
    pub(super) fn tell_questions(
        &self,
        title: impl FnOnce() -> Option<String>,
        changes: Vec<QuestionChange>,
    ) {
        if changes.is_empty() {
            return;
        }
        let title = title();
        let owed = self.owed();
        let Some(owed) = owed.as_ref() else {
            return;
        };
        for change in changes {
            match change {
                QuestionChange::Asked { request, ask } => {
                    owed.tell_waiting(title.clone(), request, ask);
                }
                QuestionChange::Closed(request) => owed.withdraw_waiting(request),
            }
        }
    }
}

impl Sessions {
    /// Makes `session`'s turn, which a prompt naming `key` just started,
    /// owe a notice to the session that registered that prompt, if one
    /// did. Call under `session`'s turn lock, right after the turn started.
    pub(super) fn claim_starter(&self, session: &LoadedSession, key: Option<&IdempotencyKey>) {
        let Some(key) = key else {
            return;
        };
        // The turn lock comes before `loaded`, never after.
        let owed = self
            .loaded()
            .values()
            .find_map(|loaded| loaded.peer_wake.inbox()?.claim(session.id.as_str(), key));
        if owed.is_some() {
            *session.peer_wake.owed() = owed;
        }
    }
}

/// A change of the questions a session's turn waits for a person to answer.
pub(super) enum QuestionChange {
    Asked {
        request: wire::InputRequestId,
        ask: String,
    },
    /// An input request closed; one that was no question withdraws nothing.
    Closed(wire::InputRequestId),
}

/// The questions `events`, those of one change of the session's input
/// requests, asked, and the requests they closed. Each request opens once.
pub(super) fn question_changes(events: &[wire::SessionEvent]) -> Vec<QuestionChange> {
    events
        .iter()
        .filter_map(|event| match event {
            wire::SessionEvent::InputRequested { request } => match &request.ask {
                wire::Ask::Question(question) => Some(QuestionChange::Asked {
                    request: request.id,
                    ask: ask_of(question),
                }),
                // Approvals, plan approvals, and what a newer protocol adds:
                // only a person answers them, and their client shows them.
                _ => None,
            },
            wire::SessionEvent::InputResolved { id, .. } => Some(QuestionChange::Closed(*id)),
            _ => None,
        })
        .collect()
}

/// What `question` asks: its title, else its first question's prompt.
fn ask_of(question: &wire::QuestionAsk) -> String {
    std::iter::once(&question.title)
        .chain(question.questions.iter().map(|asked| &asked.prompt))
        .find(|text| !text.trim().is_empty())
        .cloned()
        .unwrap_or_default()
}

/// How a turn that ended as `end` with `outcome` reads to its starter.
fn turn_end(end: &TurnEnd, outcome: Option<&wire::TurnOutcome>) -> PeerTurnEnd {
    match end {
        TurnEnd::Completed => match outcome {
            Some(wire::TurnOutcome::Answered { text, stop }) => PeerTurnEnd::Answered {
                answer: text.clone(),
                stop: *stop,
            },
            Some(wire::TurnOutcome::MaxSteps { text }) => PeerTurnEnd::StepLimit {
                reply: text.clone(),
            },
            // A completed turn records one of those two, unless the daemon
            // stops, when it records none (see `DaemonView::end`).
            Some(_) | None => PeerTurnEnd::Interrupted,
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

#[cfg(test)]
#[path = "peer_wake_tests.rs"]
mod tests;
