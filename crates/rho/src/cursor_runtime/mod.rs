//! Cursor Agent CLI (`cursor-agent`) as an external subagent runtime.
//!
//! Protocol facts (recorded 2026-09-03 against `cursor-agent 2026.08.25`):
//! `-p` mode has every tool enabled by default and `--force` changes nothing
//! there; `--exclude-tools` does not fence. Only `--allowed-tools` (snake_case
//! names such as `read_tool_call`) and `--mode plan` restrict what the child
//! may do, so spawn must always pass an explicit allow list. There is no
//! approval protocol: Rho supports Plan and Bypass permission classes only.
//! Stdin carries one prompt; follow-up turns re-spawn with `--resume`.
//!
//! Nothing here is a Rho credential; Rho never stores Cursor tokens.

// Phase D bind/execute/login; this process layer is complete but unreachable from app.
#![allow(dead_code)]

pub(crate) mod auth;
pub(crate) mod executable;
pub(crate) mod models;
pub(crate) mod session;
pub(crate) mod spawn;
pub(crate) mod stream;
