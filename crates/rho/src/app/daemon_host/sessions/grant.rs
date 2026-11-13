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
//!
//! A session that opens under a limit, as every session an agent starts
//! through session control does, is held to the grant it opened with
//! (#1796), itself no more than its parent's turn could give: its job keeps
//! that as the limit of every later grant, so a prompt from a person, or
//! from a session with more, cannot widen it. A session that opens without
//! one, as a person's does, has no limit, and each prompt grants it anew.

use std::{collections::BTreeMap, sync::Arc};

use rho_protocol::{ClientEnvironment, ErrorCode, RpcError, SessionGrant, SessionSetup};

use super::open::{session_environment, OpenWith};
use crate::{
    caller_context::{CallerContext, ENV_NAMES},
    session_environment::Ambient,
};

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

/// The grant a new session opens with when its request asks for `asked`:
/// `asked`, and when another session's turn starts it, no more than a
/// session started under `parents`, the grant of the job writing that
/// session, may have (#1796). A session with no parent, or with one no job
/// of the daemon writes, has no record to hold it to and opens with what
/// was asked.
///
/// `parents` is the grant of the parent's turn under way, or of its last
/// one under this daemon when it runs elsewhere now, which can only make
/// the new session's grant less.
pub(super) fn opening(asked: CallerContext, parents: Option<&CallerContext>) -> CallerContext {
    match parents {
        Some(parents) => asked.within(&parents.child()),
        None => asked,
    }
}

/// `environment` carrying `grant`: its variables, with the grant's in place
/// of any it had.
pub(super) fn carrying(grant: &CallerContext, environment: &Ambient) -> Ambient {
    let mut variables = match environment {
        Ambient::Client(variables) => BTreeMap::clone(variables),
        // Never a daemon session's: each runs in a client's variables.
        Ambient::ThisProcess => return Ambient::ThisProcess,
    };
    for name in ENV_NAMES {
        variables.remove(name);
    }
    // A grant without limits is carried by no variable.
    if !grant.is_root() {
        variables.extend(
            grant
                .env()
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value)),
        );
    }
    Ambient::Client(Arc::new(variables))
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
        Ok(Self {
            environment: carrying(&grant, &session_environment(environment)?),
            setup,
            grant,
        })
    }

    /// What this request brings a session it reopens whose job gave it
    /// `grant` instead of the one it asked for.
    pub(super) fn granted(&self, grant: CallerContext) -> Self {
        Self {
            environment: carrying(&grant, &self.environment),
            setup: self.setup.clone(),
            grant,
        }
    }
}

#[cfg(test)]
#[path = "grant_tests.rs"]
mod tests;
