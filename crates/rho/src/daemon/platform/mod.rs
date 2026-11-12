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
//! | Private files | [`DaemonPathError`], [`create_private_dir`], [`prepare_private_dir`], [`create_private_file`], [`sync_dir`], [`open_lock_file`], [`open_log_file`], [`FileIdentity`], [`running_identity`], [`account_lock_path`], [`lock_account_file`] |
//! | Process | [`spawn_detached`], [`MACHINE_VARIABLES`], [`is_variable`], [`request_stop`], [`StopRequests`], [`stderr_is_journal_stream`] |
//! | Restart | [`relaunch`], [`take_handoff`] |
//! | Install | [`REPLACEABLE_WHILE_RUNNING`], [`stop_service`] |
//!
//! What each must guarantee:
//!
//! - **Endpoint.** Only the user who runs the daemon can connect to what
//!   [`Listener::bind`] listens on, and [`connect`] only ever reaches a
//!   daemon of that user. `Ok(None)` from [`connect`] means nothing listens.
//! - **Private files.** A directory [`prepare_private_dir`] accepted and a
//!   file the `open_*` functions created are closed to other users.
//! - **Process.** A daemon started through [`spawn_detached`] outlives the
//!   client that started it and its terminal and, where the platform can
//!   move it, the service manager's unit the client runs in (a unit of the
//!   user's systemd manager on Linux); one it cannot move stays in that
//!   unit. [`request_stop`] reaches a daemon
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
//! - **Install.** [`REPLACEABLE_WHILE_RUNNING`] says whether an installer can put a
//!   new file at the daemon's path while the daemon runs, which decides
//!   whether an update restarts the daemon after installing or stops it
//!   before. [`stop_service`] asks the manager of a saved
//!   `rho mcp serve --http` service to stop it and not start it again.

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
    REPLACEABLE_WHILE_RUNNING,
};
pub(super) use imp::{
    connect, connect_restarting, endpoint, inspect_endpoint, is_variable, open_lock_file,
    open_log_file, prepare_private_dir, relaunch, remove_endpoint, request_stop, running_identity,
    spawn_detached, stop_service, take_handoff, Listener, Stream, MACHINE_VARIABLES,
};

#[cfg(test)]
pub(crate) use imp::test_support::StopTarget;
