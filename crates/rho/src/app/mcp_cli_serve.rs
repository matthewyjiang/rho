//! `rho mcp serve`: the boundary that reads the process environment (the
//! caller's session depth, the environment sessions run with), then hands
//! resolved values to `app::mcp_server` and `app::session_control`.

use crate::cli::McpServeArgs;

#[cfg(not(unix))]
pub(super) async fn serve(_args: &McpServeArgs) -> anyhow::Result<()> {
    anyhow::bail!(super::super::daemon_cli::UNSUPPORTED)
}

#[cfg(unix)]
pub(super) async fn serve(args: &McpServeArgs) -> anyhow::Result<()> {
    use std::sync::Arc;

    use rho_protocol::ClientEnvironment;

    use crate::app::{
        mcp_server,
        session_control::{Caller, Limits, SessionControl},
    };

    let defaults = Limits::default();
    let limits = Limits {
        max_depth: args.max_depth.unwrap_or(defaults.max_depth),
        max_sessions: args.max_sessions.unwrap_or(defaults.max_sessions),
    };
    let caller = Caller {
        depth: caller_depth()?,
        environment: ClientEnvironment::of_this_process(),
    };
    mcp_server::serve_stdio(Arc::new(SessionControl::new(caller, limits))).await
}

/// The nesting depth of the session that started this process, from
/// `RHO_SESSION_DEPTH`; 0 when no session did.
#[cfg(unix)]
fn caller_depth() -> anyhow::Result<u32> {
    use anyhow::Context;

    use crate::child_env::SESSION_DEPTH_ENV;

    match std::env::var(SESSION_DEPTH_ENV) {
        Ok(depth) => depth
            .parse()
            .with_context(|| format!("{SESSION_DEPTH_ENV}={depth:?} is not a depth")),
        Err(std::env::VarError::NotPresent) => Ok(0),
        Err(error) => Err(error).context(SESSION_DEPTH_ENV),
    }
}
