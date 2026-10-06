//! One-shot ACP sign-in for login commands: `initialize`, then `authenticate`
//! with the method the caller names. Delegated runs never authenticate here.

use super::handshake::{bounded, error};
use agent_client_protocol::{
    schema::{
        v1::{AuthenticateRequest, InitializeRequest},
        ProtocolVersion,
    },
    Client, ConnectTo,
};
use std::time::Duration;

/// Sign in through an ACP agent. `budget` bounds `authenticate` alone, which
/// waits on a human; the agent's own error text is returned unchanged.
pub(crate) async fn authenticate(
    transport: impl ConnectTo<Client> + 'static,
    method: &str,
    budget: Duration,
) -> Result<(), String> {
    let method = method.to_owned();
    Client
        .builder()
        .name("rho")
        .connect_with(transport, async move |cx| {
            let response = bounded(
                "initialize",
                cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task(),
            )
            .await?;
            if response.protocol_version != ProtocolVersion::V1 {
                return Err(error(format!(
                    "acp: initialize: unsupported protocol version {}",
                    response.protocol_version
                )));
            }
            tokio::time::timeout(
                budget,
                cx.send_request(AuthenticateRequest::new(method))
                    .block_task(),
            )
            .await
            .map_err(|_| {
                error(format!(
                    "acp: authenticate exceeded login budget {} s",
                    budget.as_secs()
                ))
            })?
            .map_err(|source| error(format!("acp: authenticate: {source}")))?;
            Ok(())
        })
        .await
        .map_err(|source| source.to_string())
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
