//! The daemon on Windows: a named pipe only the current user can open, a
//! second pipe as its stop channel, and a restart that stops this process
//! and starts another (#1665, #1778).
//!
//! The daemon's files live in the rho home, under the user's profile, and
//! are private by the profile's access control list: nothing here narrows
//! it further. The pipes are not files, so each carries its own security
//! descriptor (see `security`).

mod account;
mod endpoint;
mod files;
mod pipe;
mod process;
mod restart;
mod security;

pub(crate) use account::{account_lock_path, lock_account_file};
pub(in crate::daemon) use endpoint::{
    connect, connect_restarting, endpoint, inspect_endpoint, remove_endpoint, Listener, Stream,
};
pub(crate) use files::{create_private_dir, create_private_file, DaemonPathError, FileIdentity};
pub(in crate::daemon) use files::{
    open_lock_file, open_log_file, path_bytes, prepare_private_dir, running_identity,
};
pub(in crate::daemon) use process::{detach, is_variable, request_stop, MACHINE_VARIABLES};
pub(crate) use process::{stderr_is_journal_stream, StopRequests};
pub(in crate::daemon) use restart::{relaunch, take_handoff};

#[cfg(test)]
pub(crate) mod test_support;
