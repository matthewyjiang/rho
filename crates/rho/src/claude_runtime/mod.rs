//! Claude Code (`claude-agent-acp`) as an external subagent runtime.
//!
//! Delegated runs and the advisor speak ACP to `claude-agent-acp`, the ACP
//! adapter for the Claude Agent SDK, through the generic `acp_runtime`. The
//! adapter bundles the Claude CLI and runs it with `--cli`, which serves
//! sign-in, auth status, and the `/usage` probe ([`executable`]). Protocol
//! facts, recorded 2026-11-04 against claude-agent-acp 0.100.0 (Agent SDK
//! 0.3.316, bundled Claude Code 2.1.316); see `docs/subagents/claude-cli.md`:
//! - `_meta.claudeCode.options` reaches the Agent SDK: `tools` is the
//!   built-in allowlist, and `effort`, `maxTurns`, `settingSources`, and
//!   `persistSession` apply as given. The session model comes from
//!   `ANTHROPIC_MODEL`, then the user's `settings.model`, so a pin is set in
//!   that variable as well as `model`.
//!   `_meta.systemPrompt` replaces (a string) or extends (`append`) Claude's
//!   system prompt.
//! - The starting mode follows the user's Claude `defaultMode`, so every run
//!   sets its mode ([`fence`]). In `default` with no settings loaded, Claude
//!   runs read-only tools itself and asks Rho for anything else.
//! - Steering (`_session/steering`) injects parent messages into the running
//!   turn. ACP session ids are Claude session ids.
//! - Rate limits arrive as `usage_update._meta["_claude/rateLimit"]`.
//! - A signed-out (or expired) login still reports an account in
//!   `_auth/status_update` and fails only at `session/prompt` with
//!   "Authentication required"; `claude auth status` is the reliable check.
//!
//! Nothing here is a Rho credential; Rho never stores Claude Code tokens.

pub(crate) mod auth;
pub(crate) mod executable;
pub(crate) mod fence;
pub(crate) mod models;
pub(crate) mod one_shot;
mod policy;
pub(crate) mod rate_limit;
pub(crate) mod rate_limit_display;
pub(crate) mod session;
pub(crate) mod tools;
#[cfg(any(unix, test))]
pub(crate) mod usage_parse;
pub(crate) mod usage_probe;
#[cfg(unix)]
pub(crate) mod usage_pty;
pub(crate) mod window_kind;
