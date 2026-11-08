//! Mechanics the rho daemon's clients (`rho acp`, `rho attach`) share: the
//! connection and its notification router ([`Daemon`]), a client's place
//! in a session's event stream ([`StreamPosition`]), what a reset's replay
//! tells of the last turn ([`ResetReplay`]), and the Herdr pane state a
//! session's job shows ([`HerdrStatus`]).
//!
//! What a client does with a session's events (ACP updates, `rho attach`'s
//! output and exit) stays with that client.

mod connection;
mod herdr;
mod position;
mod replay;
#[cfg(test)]
pub(crate) mod test_support;

pub(crate) use connection::{Daemon, DaemonError, Link, LoginEvent, Routed, SessionRoute};
pub(crate) use herdr::HerdrStatus;
pub(crate) use position::{Part, StreamPosition};
pub(crate) use replay::{ReplayedTurn, ResetReplay};
