//! The grant a request brings a session, which the daemon holds on the
//! session's job (#1780): what the session's turn may do through session
//! control (`crate::caller_context`).
//!
//! A request names its grant, as the session-control core's do, or leaves it
//! to its client's environment, as a person's `rho run --detach` inside a
//! session's shell does. Either way the daemon decides the grant once,
//! records it, and writes the session's `RHO_SESSION_*` variables from it,
//! when it opens or reopens the session and when a prompt starts a turn, so
//! the record and what the session's processes see never differ.

use rho_protocol::{ClientEnvironment, ErrorCode, RpcError, SessionGrant, SessionSetup};

use super::open::{session_environment, OpenWith};
use crate::caller_context::{CallerContext, ENV_NAMES};

/// The grant a request asks for: the one it names, else the one its
/// client's `environment` carries. A grant the environment carries
/// malformed is invalid params, never the unlimited default.
pub(super) fn asked(
    named: Option<SessionGrant>,
    environment: &ClientEnvironment,
) -> Result<CallerContext, RpcError> {
    match named {
        Some(grant) => Ok(grant.into()),
        None => CallerContext::from_env(|name| {
            environment
                .iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| value.into())
        })
        .map_err(|error| RpcError::new(ErrorCode::InvalidParams, format!("{error:#}"))),
    }
}

impl OpenWith {
    /// What a request with `environment` and `setup` brings a session it
    /// runs under `grant`: the client's variables, with the grant's in place
    /// of any the client's carry.
    pub(super) fn under(
        grant: CallerContext,
        environment: &ClientEnvironment,
        setup: SessionSetup,
    ) -> Result<Self, RpcError> {
        // A grant without limits is carried by no variable.
        let carried = if grant.is_root() {
            Vec::new()
        } else {
            grant.env()
        };
        let environment: ClientEnvironment = environment
            .iter()
            .filter(|(name, _)| !ENV_NAMES.contains(name))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .chain(
                carried
                    .into_iter()
                    .map(|(name, value)| (name.to_owned(), value)),
            )
            .collect();
        Ok(Self {
            environment: session_environment(&environment)?,
            setup,
            grant,
        })
    }
}
