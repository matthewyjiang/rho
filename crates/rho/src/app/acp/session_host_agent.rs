//! `/agent:<name>` for ACP sessions: the runtime rebinds the fresh session
//! to the delegated agent, the host advertises what the agent changed, then
//! runs the task as the child's first turn.

use agent_client_protocol::schema::v1::PromptResponse;
use rho_sdk::UserInput;

use super::{AcpClientPort, AcpStartup, SessionHost, Shown};
use crate::app::session_runtime::{AgentTask, PromptScope, Turn};

impl SessionHost {
    /// Rebinds a fresh child for `task`, then submits only the task to the
    /// model as `prompt`'s turn. The original command remains in display
    /// history so the host can show how the child was launched.
    pub(super) async fn run_as_agent(
        &mut self,
        prompt: &PromptScope,
        display: &UserInput,
        task: AgentTask,
        startup: &AcpStartup,
        client: &dyn AcpClientPort,
    ) -> Shown<PromptResponse> {
        if let Err(error) = self
            .runtime
            .rebind_agent(prompt, &task, &startup.config)
            .await
        {
            return Shown::delivered(Err(error));
        }
        self.run_rebound_agent(prompt, display, task, startup, client)
            .await
    }

    /// [`Self::run_as_agent`] in a prompt of its own, with the binding and
    /// build injected, so tests can rebind on a scripted session without
    /// credentials.
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
            Output = anyhow::Result<(
                crate::app::session_assembly::BuiltSession,
                crate::app::session_runtime::SessionPermission,
            )>,
        >,
    {
        let prompt = self
            .runtime
            .begin_prompt(&startup.config, &startup.config_path)
            .await?;
        self.runtime
            .rebind_agent_with(&prompt, &task, &startup.config, bind, build)
            .await?;
        self.run_rebound_agent(&prompt, display, task, startup, client)
            .await
            .into_result()
    }

    async fn run_rebound_agent(
        &mut self,
        prompt: &PromptScope,
        display: &UserInput,
        task: AgentTask,
        startup: &AcpStartup,
        client: &dyn AcpClientPort,
    ) -> Shown<PromptResponse> {
        self.refresh_advertisements(&startup.config, client).await;
        let turn = Turn {
            display: display.clone(),
            model: task.input,
            calls: Vec::new(),
        };
        self.run_turn(prompt, turn, &startup.config, client).await
    }
}

#[cfg(test)]
#[path = "session_host_agent_tests.rs"]
mod tests;
