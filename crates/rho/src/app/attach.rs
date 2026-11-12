//! `rho attach <ID>`: follows a daemon session's turn over `rho-protocol`
//! until it ends (#1669), and exits as the turn ended, with `rho run`'s exit
//! codes. A session job's id or its session's id names the session. With
//! no turn under way, it reports the session's last turn; before the
//! session's first prompt, it waits for that turn.
//!
//! Output is text or the JSON Lines stream `rho run --output jsonl` writes:
//! `run_started`, the session's events from the daemon's replay on, and
//! `run_ended`. In text mode, lines typed at the terminal answer the turn's
//! input requests, steer it, or cancel it. Ctrl+C detaches: the connection
//! closes, and the turn runs on in the daemon.

mod client;
mod console;
mod follow;
mod prompt;
mod terminal;
mod text;

pub(super) use client::run;
