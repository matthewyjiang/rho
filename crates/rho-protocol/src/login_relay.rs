//! The channel between one login the daemon runs and the connection that
//! started it (#1662).
//!
//! The daemon keeps the [`LoginRelay`] in the login's task and hands the
//! matching [`LoginStream`] to [`crate::serve_connection`] in its
//! [`crate::Service::login`] answer. The connection forwards the stream as
//! [`crate::methods::AUTH_LOGIN_PROMPT`] and
//! [`crate::methods::AUTH_LOGIN_FINISHED`] notifications, and drops it when
//! it closes, which the relay sees as [`LoginRelay::closed`]: the login has
//! nobody left to show its prompt to.

use tokio::sync::mpsc::{self, error::TryRecvError};

use crate::methods::{LoginOutcome, LoginPrompt};

/// Opens the relay of one login to auth profile `auth`.
///
/// Unbounded, yet bounded by construction: a login sends one prompt (each
/// provider flow in `rho-providers`' `login_dispatch` shows one) and then
/// its outcome, which consumes the relay.
pub fn login_relay(auth: impl Into<String>) -> (LoginRelay, LoginStream) {
    let (events, receiver) = mpsc::unbounded_channel();
    let relay = LoginRelay {
        auth: auth.into(),
        events,
    };
    (relay, LoginStream { events: receiver })
}

/// The daemon's end of one login's relay.
#[derive(Debug)]
pub struct LoginRelay {
    auth: String,
    events: mpsc::UnboundedSender<LoginEvent>,
}

/// The relay's connection is gone, so nothing sent reaches a client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the client that started the login is gone")]
pub struct RelayGone;

impl LoginRelay {
    /// Sends what the user must do to the login's client.
    pub fn prompt(&self, prompt: LoginPrompt) -> Result<(), RelayGone> {
        self.events
            .send(LoginEvent::Prompt(prompt))
            .map_err(|_| RelayGone)
    }

    /// Sends how the login ended, its last message.
    pub fn finish(self, outcome: LoginOutcome) -> Result<(), RelayGone> {
        let Self { auth, events } = self;
        events
            .send(LoginEvent::Finished { auth, outcome })
            .map_err(|_| RelayGone)
    }

    /// Resolves once the connection that started the login is gone.
    pub async fn closed(&self) {
        self.events.closed().await;
    }
}

/// The connection's end of one login's relay.
#[derive(Debug)]
pub struct LoginStream {
    events: mpsc::UnboundedReceiver<LoginEvent>,
}

/// One message a login sends its client.
#[derive(Debug)]
pub(crate) enum LoginEvent {
    Prompt(LoginPrompt),
    Finished { auth: String, outcome: LoginOutcome },
}

impl LoginStream {
    /// The next message, or `None` once the relay is gone and every message
    /// it sent was taken.
    pub(crate) async fn next(&mut self) -> Option<LoginEvent> {
        self.events.recv().await
    }

    /// The next message already sent, without waiting; `None` when there is
    /// none yet or the relay is gone.
    pub(crate) fn try_next(&mut self) -> Option<LoginEvent> {
        match self.events.try_recv() {
            Ok(event) => Some(event),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }
}
