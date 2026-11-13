//! Mechanics the rho daemon's clients (`rho acp`, `rho attach`, and the
//! TUI's daemon sessions, `tui::daemon_session`) share: the
//! connection and its notification router ([`Daemon`]), a client's place
//! in a session's event stream ([`StreamPosition`]), and the Herdr pane
//! state a session's job shows ([`HerdrStatus`]).
//!
//! What a client does with a session's events (ACP updates, `rho attach`'s
//! output and exit) stays with that client. Each tells the turn it follows
//! from any other by the turn's id, which the daemon answers a prompt with
//! and puts on the turn's states and its end (#1777).

mod connection;
mod herdr;
mod position;
#[cfg(test)]
pub(crate) mod test_support;

pub(crate) use connection::{Daemon, DaemonError, Link, LoginEvent, Routed, SessionRoute};
pub(crate) use herdr::HerdrStatus;
pub(crate) use position::{Part, StreamPosition};
