//! Generic ACP client mechanics: transport, handshake, turns, and run artifacts.
//!
//! Agent-specific argv, configuration, permission policy, and extension answers
//! belong to the owning runtime, never to this driver.

mod driver;
mod handshake;
pub(crate) mod permission;
pub(crate) mod policy;
mod session;
mod transport;
mod turn;

pub(crate) use policy::{AcpAgentPolicy, AcpSpawnPlan, ExtensionAnswer};
pub(crate) use session::AcpSessionRequest;

/// Run one delegated ACP process through the shared artifact boundary.
/// The implementation mechanics and single terminal-write gate live in session.
pub(crate) async fn run_session<P: AcpAgentPolicy>(
    mut request: AcpSessionRequest,
    policy: P,
) -> anyhow::Result<()> {
    let mut sink = session::open_sink(&mut request, &policy)?;
    let mut renderer =
        crate::cli_runtime::agent_event::render::EventRenderer::new(request.cwd.clone());
    let outcome = session::run_child(&mut request, policy, &mut renderer, &mut sink).await;
    if let Some(inbox) = request.parent_messages.as_ref() {
        inbox.seal();
    }
    session::settle(sink, outcome).await;
    Ok(())
}

// Phase 5 widens this to debug builds for the hidden fixture subcommand.
#[cfg(test)]
pub(crate) mod scripted_agent;

#[cfg(test)]
mod test_support;

#[cfg(all(test, unix))]
#[path = "process_tests.rs"]
mod process_tests;
#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
