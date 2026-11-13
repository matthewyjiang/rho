//! The daemon on Windows: a named pipe only the current user can open, a
//! second pipe as its stop channel, and a restart that stops this process
//! and starts another, which waits for this one to exit (#1665, #1778).
//!
//! The daemon's files live in the rho home, under the user's profile, and
//! are private by the profile's access control list: nothing here narrows
//! it further. The pipes are not files, so each carries its own security
//! descriptor (see `security`).

mod account;
mod endpoint;
mod files;
mod memory;
mod pipe;
mod process;
mod restart;
mod security;
mod service;

pub(crate) use account::{account_lock_path, lock_account_file};
pub(in crate::daemon) use endpoint::{
    connect, connect_restarting, inspect_endpoint, remove_endpoint, Listener, Stream,
};
pub(crate) use files::{
    create_private_dir, create_private_file, sync_dir, DaemonPathError, FileIdentity,
    REPLACEABLE_WHILE_RUNNING,
};
pub(in crate::daemon) use files::{
    open_lock_file, open_log_file, prepare_private_dir, running_identity,
};
pub(crate) use memory::release_freed_memory;
pub(in crate::daemon) use process::{is_variable, request_stop, spawn_detached, MACHINE_VARIABLES};
pub(crate) use process::{stderr_is_journal_stream, StopRequests};
pub(in crate::daemon) use restart::{relaunch, take_handoff};
pub(in crate::daemon) use service::stop_service;

#[cfg(test)]
pub(crate) mod test_support;
