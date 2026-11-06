//! `/agent:<name>` for ACP sessions: the runtime rebinds the fresh session
//! to the delegated agent, the host advertises what the agent changed, then
//! runs the task as the child's first turn.

use agent_client_protocol::schema::v1::PromptResponse;
use rho_sdk::UserInput;

use super::{AcpClientPort, AcpStartup, SessionHost};
use crate::app::session_runtime::AgentTask;

impl SessionHost {
    /// Rebinds a fresh child for `task`, then submits only the task to the
    /// model. The original command remains in display history so the host
    /// can show how the child was launched.
    pub(super) async fn run_as_agent(
        &mut self,
        display: &UserInput,
        task: AgentTask,
        startup: &AcpStartup,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        self.runtime.rebind_agent(&task, &startup.config).await?;
        self.run_rebound_agent(display, task, startup, client).await
    }

    /// [`Self::run_as_agent`] with the binding and build injected, so tests
    /// can rebind on a scripted assembly without credentials.
    #[cfg(test)]
    async fn run_as_agent_with<Build, BuildFuture>(
        &mut self,
        display: &UserInput,
        task: AgentTask,
        startup: &AcpStartup,
        client: &dyn AcpClientPort,
        bind: impl FnOnce(
            &crate::config::Config,
        ) -> anyhow::Result<crate::app::agent_binding::BoundAgent>,
        build: Build,
    ) -> anyhow::Result<PromptResponse>
    where
        Build: FnOnce(
            crate::app::agent_binding::BoundAgent,
            crate::config::Config,
            rho_sdk::SessionId,
        ) -> BuildFuture,
        BuildFuture: std::future::Future<
            Output = anyhow::Result<crate::app::session_assembly::SessionAssembly>,
        >,
    {
        self.runtime
            .rebind_agent_with(&task, &startup.config, bind, build)
            .await?;
        self.run_rebound_agent(display, task, startup, client).await
    }

    async fn run_rebound_agent(
        &mut self,
        display: &UserInput,
        task: AgentTask,
        startup: &AcpStartup,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        self.refresh_advertisements(&startup.config, client).await;
        self.run_turn(display, task.input, Vec::new(), client).await
    }
}

#[cfg(test)]
#[path = "session_host_agent_tests.rs"]
mod tests;
