//! Claude Code as an external subagent runtime.
//!
//! Auth and binary probes live here. Nothing in this module is a Rho
//! credential; Rho never stores Claude Code tokens.

pub(crate) mod auth;
pub(crate) mod executable;
pub(crate) mod line_decoder;
pub(crate) mod persist;
pub(crate) mod rate_limit;
pub(crate) mod session;
pub(crate) mod spawn;
pub(crate) mod stream;
pub(crate) mod windows_shim_args;
