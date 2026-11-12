//! The daemon's platform boundary: everything that differs between the
//! operating systems the daemon runs on. The rest of `crate::daemon`, the
//! daemon's host, and its clients are written against the items re-exported
//! here and carry no `cfg` of their own.
//!
//! Each platform module exports the same items with the same signatures:
//!
//! | Concern | Items |
//! |---|---|
//! | Endpoint | [`endpoint`], [`Listener`], [`Stream`], [`connect`], [`connect_restarting`], [`remove_endpoint`], [`inspect_endpoint`] |
//! | Private files | [`DaemonPathError`], [`create_private_dir`], [`prepare_private_dir`], [`create_private_file`], [`sync_dir`], [`open_lock_file`], [`open_log_file`], [`path_bytes`], [`FileIdentity`], [`running_identity`], [`account_lock_path`], [`lock_account_file`] |
//! | Process | [`detach`], [`MACHINE_VARIABLES`], [`is_variable`], [`request_stop`], [`StopRequests`], [`stderr_is_journal_stream`] |
//! | Restart | [`relaunch`], [`take_handoff`] |
//!
//! What each must guarantee:
//!
//! - **Endpoint.** Only the user who runs the daemon can connect to what
//!   [`Listener::bind`] listens on, and [`connect`] only ever reaches a
//!   daemon of that user. `Ok(None)` from [`connect`] means nothing listens.
//! - **Private files.** A directory [`prepare_private_dir`] accepted and a
//!   file the `open_*` functions created are closed to other users.
//! - **Process.** A daemon started through [`detach`] outlives the client
//!   that started it and its terminal. [`request_stop`] reaches a daemon
//!   that cannot take a protocol request, and that daemon hears it from
//!   [`StopRequests`] and shuts down as gracefully as on `rho/daemon/stop`.
//! - **Restart.** [`relaunch`] replaces a daemon that stopped serving with a
//!   new run of its executable. A platform that can keep the lock and the
//!   listener across the restart passes them as a handoff token, which the
//!   new run gives back to [`take_handoff`]; clients then wait in the
//!   endpoint's backlog. One that cannot releases both and starts another
//!   process, whose token names what [`take_handoff`] waits for before it
//!   returns the lock file alone, for the new run to lock and bind as any
//!   daemon does, and [`connect_restarting`] waits for it instead. Either way
//!   `Listener::hold_for_restart` leaves no connection open to a client
//!   that nothing will serve.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

pub(crate) use imp::{
    account_lock_path, create_private_dir, create_private_file, lock_account_file,
    stderr_is_journal_stream, sync_dir, DaemonPathError, FileIdentity, StopRequests,
};
pub(super) use imp::{
    connect, connect_restarting, detach, endpoint, inspect_endpoint, is_variable, open_lock_file,
    open_log_file, path_bytes, prepare_private_dir, relaunch, remove_endpoint, request_stop,
    running_identity, take_handoff, Listener, Stream, MACHINE_VARIABLES,
};

#[cfg(test)]
pub(crate) use imp::test_support::StopTarget;

/// Where a rho home's daemon listens, as [`endpoint`] lays it out.
pub(super) struct Endpoint {
    /// The private directory that holds the spawn lock and, where the
    /// endpoint is a file, the endpoint.
    pub(super) runtime_dir: std::path::PathBuf,
    /// What clients connect to, in the form the daemon reports it.
    pub(super) socket: std::path::PathBuf,
}
