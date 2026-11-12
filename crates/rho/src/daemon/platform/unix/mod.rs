//! The daemon on Unix: a socket file in a 0700 directory, descriptors kept
//! across an `exec` to restart in place, and signals to stop.

mod endpoint;
mod files;
mod handoff;
mod process;

pub(in crate::daemon) use endpoint::{
    connect, connect_restarting, endpoint, inspect_endpoint, remove_endpoint, Listener, Stream,
};
pub(in crate::daemon) use files::{
    create_private_dir, open_lock_file, open_log_file, path_bytes, prepare_private_dir,
    running_identity,
};
pub(crate) use files::{DaemonPathError, FileIdentity};
pub(in crate::daemon) use handoff::{relaunch, take_handoff};
pub(in crate::daemon) use process::{detach, is_variable, request_stop, MACHINE_VARIABLES};
pub(crate) use process::{stderr_is_journal_stream, StopRequests};

#[cfg(test)]
#[path = "paths_tests.rs"]
mod paths_tests;
