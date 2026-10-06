//! Google Antigravity (`agy_acp_server`) as an external subagent runtime.
//!
//! Delegated runs speak ACP through the generic `acp_runtime`. Protocol facts
//! (recorded 2026-10-06 against agy_acp_server 1.3.0, see
//! `docs/subagents/antigravity.md`): there is no plan mode, so Rho requires
//! mode `default`, narrows built-ins with `_meta.agy.enabledTools`, and answers
//! every permission request itself. Rho supports Plan and Bypass only.
//!
//! Nothing here is a Rho credential: the server owns its sign-in under the
//! Gemini home, and Rho only checks that one exists.

pub(crate) mod executable;
pub(crate) mod fence;
pub(crate) mod home;
pub(crate) mod login;

pub(crate) mod policy;
pub(crate) mod session;
pub(crate) mod setup;

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
