//! Continuing the session turns a restart in place interrupted (`rho daemon
//! restart`, or an upgrade's re-execution).
//!
//! The restarting daemon marks the job of each session whose turn its stop
//! interrupts, if the turn may be continued (see `jobs`): only a turn with
//! no time limit, of a session whose client takes the turns the daemon
//! starts (`idle_wake: turn`, see [`super::idle_wake`]). The turn that
//! continues it is one the daemon starts, with no limit, so a session
//! opened to take no such turns, as `rho run --detach`'s is, or a turn
//! that had a limit, stays interrupted. The daemon the restart re-executes
//! takes the marks as it starts, and every daemon start clears them, so a
//! turn is continued at most once and only right after the restart that
//! cut it short: a stop, a crash, a signal, or a client's cancel never
//! continues one.
//!
//! The restarted daemon continues a session's turn once a client attaches
//! to the session, rather than reopening the session itself: only a
//! client's request brings what the session runs with, its environment,
//! credentials among them (#1659, #1779), and its setup, and neither ever
//! reaches disk. So the continuing turn runs in the environment of the
//! client whose attach reopened the session, which must take the daemon's
//! turns too. The TUI attaches again on its own once told the daemon is
//! restarting. A turn that starts first, such as a prompt's, takes the
//! continuing turn's place.

use std::{
    collections::HashSet,
    sync::{Mutex, MutexGuard, PoisonError},
};

use rho_protocol::SessionSetup;
use rho_sdk::{SessionId, UserInput};

use super::{
    start::{Entered, StartedTurn},
    DaemonTurn, LoadedSession, OpenWith, Sessions, TurnSlot,
};
use crate::app::session_runtime::{Turn, TurnStart};

/// What the model reads as the turn that continues an interrupted one
/// begins, as an approved plan's implementation turn begins with the
/// host's message. The session saves it as the turn's user message, which
/// a later replay of the saved conversation shows as one; live, clients
/// see [`RESUME_NOTICE`] instead.
const RESUME_NOTE: &str = "The rho daemon restarted and interrupted your previous turn. \
                           Continue where you left off. A tool call the restart interrupted \
                           may have partly run; check its effects before repeating it.";

/// What clients following the session are told as the continuing turn
/// begins.
const RESUME_NOTICE: &str = "rho daemon restarted mid-turn; continuing the interrupted turn";

/// The sessions whose turn the restart that ran this daemon interrupted, and
/// that wait for a client to attach to continue it.
#[derive(Default)]
pub(super) struct RestartResumes(Mutex<HashSet<SessionId>>);

impl RestartResumes {
    /// Whether `session`'s interrupted turn waited to continue; it waits no
    /// more.
    pub(super) fn take(&self, session: &SessionId) -> bool {
        self.lock().remove(session)
    }

    fn lock(&self) -> MutexGuard<'_, HashSet<SessionId>> {
        // Only inserted into and removed from whole.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Sessions {
    /// Continues the turns of `sessions`, which the restart that ran this
    /// daemon interrupted, as clients attach to them. Call once, as the
    /// daemon starts.
    pub(in crate::app::daemon_host) fn resume_after_restart(&self, sessions: Vec<SessionId>) {
        if sessions.is_empty() {
            return;
        }
        tracing::info!(
            sessions = sessions.len(),
            "the restart interrupted turns; each continues once a client attaches to its session"
        );
        self.restart_resumes.lock().extend(sessions);
    }

    /// Starts the turn that continues `session`'s interrupted one, if it
    /// waits to and the session's client takes turns the daemon starts, in
    /// the environment and under the grant the attach that reopened the
    /// session brought. `turn` is the session's turn lock, held. The caller
    /// runs the turn it returns, a [`resume_turn`], in a
    /// [`super::TurnTask`], at once.
    pub(super) fn resume_interrupted(
        &self,
        session: &LoadedSession,
        turn: &mut TurnSlot,
    ) -> Option<StartedTurn> {
        // The first attach decides: the session's settings do not change
        // while it stays loaded, so a later one would decide the same.
        if !self.restart_resumes.take(&session.id) {
            return None;
        }
        let skipped = |reason: &str| {
            tracing::info!(
                session = %session.id,
                job = %session.job,
                reason,
                "not continuing the turn the restart interrupted"
            );
        };
        if session.idle_wake.is_none() {
            skipped("the client that reopened the session takes no turns the daemon starts");
            return None;
        }
        let Some(grant) = self.jobs.grant_of(&session.job) else {
            skipped("the session's job is not recorded");
            return None;
        };
        let with = OpenWith {
            environment: session.environment.current(),
            // Unread: `start_turn` takes the environment and grant alone,
            // the session being open already.
            setup: SessionSetup::default(),
            grant,
        };
        match self.start_turn(session, turn, &with, /*key*/ None, /*limit*/ None) {
            Ok(Entered::Started(started)) => {
                tracing::info!(
                    session = %session.id,
                    job = %session.job,
                    turn = %started.id,
                    "continuing the turn the restart interrupted"
                );
                Some(started)
            }
            // Neither comes of a turn that names no key in a session that
            // takes the daemon's turns, which never runs a wake wait.
            Ok(Entered::SentAgain { .. } | Entered::BehindWakeWait) => None,
            Err(error) => {
                tracing::warn!(
                    session = %session.id,
                    job = %session.job,
                    %error,
                    "could not continue the turn the restart interrupted"
                );
                None
            }
        }
    }
}

/// The daemon turn that continues an interrupted one: [`RESUME_NOTE`] for
/// the model, [`RESUME_NOTICE`] for clients.
pub(super) fn resume_turn() -> DaemonTurn {
    let note = UserInput::text(RESUME_NOTE);
    DaemonTurn {
        start: TurnStart::Prompt(Turn {
            display: note.clone(),
            model: note,
            calls: Vec::new(),
        }),
        notice: Some(RESUME_NOTICE.to_owned()),
    }
}
