//! What a connection declared in its `rho/hello`, and what that lets it do
//! (#1780). The one place that decides which capabilities a role may
//! declare and which methods need one.

use std::collections::BTreeSet;

use crate::{
    methods::{HELLO, SESSION_ANSWER, SESSION_HOLD},
    ClientCapability, ClientInfo, ClientRole, ErrorCode, HelloParams, RpcError,
};

/// The role and capabilities an accepted hello declared, which hold until
/// its connection ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Declared {
    role: ClientRole,
    capabilities: BTreeSet<ClientCapability>,
}

impl Declared {
    /// What `hello` declares, with the client it names, unless it declares a
    /// capability its role may not, which is invalid params.
    pub(crate) fn accept(hello: HelloParams) -> Result<(Self, ClientInfo), RpcError> {
        let HelloParams {
            protocol: _,
            client,
            role,
            capabilities,
        } = hello;
        if let Some(refused) = capabilities
            .iter()
            .find(|capability| !may_declare(role, **capability))
        {
            return Err(RpcError::new(
                ErrorCode::InvalidParams,
                format!(
                    "a connection of role `{}` may not declare capability `{}`",
                    role_name(role),
                    capability_name(*refused)
                ),
            ));
        }
        Ok((Self { role, capabilities }, client))
    }

    /// Refuses `method` when it needs a capability this connection did not
    /// declare. It reads no params, so the refusal comes before they are
    /// decoded.
    pub(crate) fn permit(&self, method: &str) -> Result<(), RpcError> {
        match needed_by(method).filter(|needed| !self.capabilities.contains(needed)) {
            Some(needed) => Err(RpcError::new(
                ErrorCode::CapabilityRequired,
                format!(
                    "{method} needs capability `{}`, which this connection, of role `{}`, did not declare in {HELLO}",
                    capability_name(needed),
                    role_name(self.role)
                ),
            )),
            None => Ok(()),
        }
    }
}

/// Whether a connection of `role` may declare `capability`.
fn may_declare(role: ClientRole, capability: ClientCapability) -> bool {
    match (role, capability) {
        (ClientRole::Frontend, ClientCapability::Answer) => true,
        (ClientRole::Agent, ClientCapability::Answer) => false,
    }
}

/// The capability `method` needs, if it needs one. A new method that acts
/// for a person, as an answer does, belongs here.
fn needed_by(method: &str) -> Option<ClientCapability> {
    match method {
        SESSION_ANSWER | SESSION_HOLD => Some(ClientCapability::Answer),
        _ => None,
    }
}

/// `role` as the wire names it.
fn role_name(role: ClientRole) -> &'static str {
    match role {
        ClientRole::Frontend => "frontend",
        ClientRole::Agent => "agent",
    }
}

/// `capability` as the wire names it.
fn capability_name(capability: ClientCapability) -> &'static str {
    match capability {
        ClientCapability::Answer => "answer",
    }
}
