//! Claude Code as an external subagent runtime.
//!
//! Auth and binary probes live here. Nothing in this module is a Rho
//! credential; Rho never stores Claude Code tokens.

pub(crate) mod auth;
pub(crate) mod executable;
pub(crate) mod messaging;
pub(crate) mod models;
pub(crate) mod one_shot;
pub(crate) mod rate_limit;
pub(crate) mod session;
pub(crate) mod spawn;
pub(crate) mod stream;
pub(crate) mod usage_parse;
pub(crate) mod usage_probe;
#[cfg(unix)]
pub(crate) mod usage_pty;
pub(crate) mod window_kind;
