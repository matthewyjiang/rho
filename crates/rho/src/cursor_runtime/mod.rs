//! Cursor Agent CLI (`cursor-agent`) as an external subagent runtime.
//!
//! Delegated runs speak ACP (`cursor-agent --trust [--model M] acp`) through
//! the generic `acp_runtime`. Protocol facts (recorded 2026-10-05 against
//! `cursor-agent 2026.10.01`, see `acp_runtime/fixtures/README.md`):
//! `--allowed-tools` and `--mode` are ignored under `acp`, and whether Cursor
//! asks permission depends on its own config. Rho therefore points
//! `CURSOR_CONFIG_DIR` at a per-run managed config that fences tools, sets plan
//! mode with `session/set_mode`, and answers permission requests by policy.
//! Rho supports Plan and Bypass permission classes only.
//!
//! Nothing here is a Rho credential; Rho never stores Cursor tokens.

pub(crate) mod acp_config;
pub(crate) mod acp_extensions;
pub(crate) mod acp_permissions;
pub(crate) mod acp_policy;
pub(crate) mod auth;
pub(crate) mod executable;
pub(crate) mod models;
pub(crate) mod session;
pub(crate) mod spawn;
