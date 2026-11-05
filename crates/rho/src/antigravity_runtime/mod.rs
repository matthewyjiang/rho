//! Google Antigravity (`agy acp`) as an external subagent runtime.
//!
//! Delegated runs speak ACP to `agy acp` (agy 1.5.0 and later) through the
//! generic `acp_runtime`. Protocol facts, recorded 2026-11-04 against agy
//! 1.5.0 (see `docs/subagents/antigravity.md`):
//! - `session/new` has no tool allowlist; an agy custom agent's `tools:` is
//!   one, so each run writes its own agent ([`agent_file`]) and fences by it,
//!   the session mode, and permission answers ([`fence`]).
//! - Mode `default` asks before commands, but honors the user's agy
//!   `toolPermission` setting, which can approve everything without asking.
//!   Runs pass `--cli_dir_name` so agy reads Rho's own (default) settings
//!   instead ([`executable::server_args`]).
//! - Sign-in belongs to agy and is shared with the agy CLI; `agy acp --login`
//!   signs in from a terminal ([`auth`]).

pub(crate) mod agent_file;
pub(crate) mod auth;
pub(crate) mod executable;
pub(crate) mod fence;
pub(crate) mod policy;
pub(crate) mod session;

use crate::cli_runtime::status_sink::RuntimeLabel;

/// Source token, error prefix, and [`RuntimeLabel::program`].
pub(crate) const ANTIGRAVITY_LABEL_NAME: &str = "antigravity";

/// Starting activity and program name for the shared artifact sink.
pub(crate) const ANTIGRAVITY_LABEL: RuntimeLabel = RuntimeLabel {
    starting_activity: "starting antigravity",
    program: ANTIGRAVITY_LABEL_NAME,
    // The ACP sessionId is protocol metadata; Rho offers no resume command.
    resume_command: None,
    session_label: "antigravity session",
    cost_label: "antigravity cost",
};
