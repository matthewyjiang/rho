use std::{path::PathBuf, sync::Arc};

use {
    crate::agent::{AgentCapabilities, AgentDefinition, ToolCapability},
    crate::cli::OutputFormat,
    crate::config::Config,
    crate::diagnostics::RuntimeDiagnostics,
    crate::herdr::HerdrReporter,
    crate::subagent::{self, RunState, RunStatus},
    rho_tools::cancellation::RunCancellation,
};

use super::{
    agent_binding::{AgentBinder, AgentInvocation, AgentRole},
    automation::{self, RunArtifactIdentity, RunReporter},
    subagent_host_input::SubagentHostInputBridge,
};

#[derive(Clone)]
pub(crate) struct AgentExecutor {
    config: Arc<std::sync::RwLock<Config>>,
    config_path: PathBuf,
    cwd: PathBuf,
    permits: Arc<tokio::sync::Semaphore>,
    host_input: SubagentHostInputBridge,
}

pub(crate) struct AgentLaunchRequest {
    pub(crate) definition: Arc<AgentDefinition>,
    pub(crate) prompt: String,
    pub(crate) run_id: String,
    pub(crate) background: bool,
    pub(crate) parent_session_id: Option<rho_sdk::SessionId>,
    pub(crate) output_file: PathBuf,
}

#[derive(Clone)]
pub(crate) struct AgentRunHandle {
    cancellation: RunCancellation,
    status: tokio::sync::watch::Receiver<RunStatus>,
    completion: tokio::sync::watch::Receiver<bool>,
}

impl AgentRunHandle {
    pub(crate) fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub(crate) fn status(&self) -> RunStatus {
        self.status.borrow().clone()
    }

    pub(crate) fn is_complete(&self) -> bool {
        *self.completion.borrow()
    }

    pub(crate) async fn wait(&mut self) -> RunStatus {
        while !*self.completion.borrow() {
            if self.completion.changed().await.is_err() {
                break;
            }
        }
        self.status()
    }
}

impl AgentExecutor {
    pub(crate) fn new(
        config: Config,
        config_path: PathBuf,
        cwd: PathBuf,
        host_input: SubagentHostInputBridge,
    ) -> Self {
        let concurrency = std::env::var("RHO_AGENT_CONCURRENCY")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|limit| *limit > 0)
            .unwrap_or(4);
        Self {
            config: Arc::new(std::sync::RwLock::new(config)),
            config_path,
            cwd,
            permits: Arc::new(tokio::sync::Semaphore::new(concurrency)),
            host_input,
        }
    }

    pub(crate) fn host_input(&self) -> &SubagentHostInputBridge {
        &self.host_input
    }

    pub(crate) fn update_model(
        &self,
        provider: &str,
        model: &str,
        reasoning: rho_sdk::ReasoningLevel,
    ) {
        let mut config = self.config.write().expect("delegated config lock");
        config.provider = provider.to_string();
        config.model = model.to_string();
        config.reasoning = reasoning;
    }

    pub(crate) fn update_permission_mode(&self, mode: crate::permission::PermissionMode) {
        self.config
            .write()
            .expect("delegated config lock")
            .permission_mode = mode;
    }

    #[cfg(test)]
    pub(crate) fn launch_permission_mode(&self) -> crate::permission::PermissionMode {
        self.config
            .read()
            .expect("delegated config lock")
            .permission_mode
    }

    pub(crate) fn spawn(&self, request: AgentLaunchRequest) -> anyhow::Result<AgentRunHandle> {
        let config = self.config.read().expect("delegated config lock").clone();
        let mut capabilities = AgentCapabilities::all_host_tools();
        if !crate::tools::web::access_tools(&config).is_available() {
            capabilities.remove(&ToolCapability::WebSearch);
        }
        #[cfg(windows)]
        capabilities.remove(&ToolCapability::Bash);
        #[cfg(not(windows))]
        capabilities.remove(&ToolCapability::Powershell);
        // A foreground child runs inside the parent tool call, so waiting for
        // that parent to present a questionnaire would deadlock both runs.
        let questionnaire_available =
            request.background && request.parent_session_id.is_some() && self.host_input.is_bound();
        if !questionnaire_available {
            capabilities.remove(&ToolCapability::Questionnaire);
        }
        let bound = AgentBinder::bind(
            request.definition,
            AgentInvocation {
                role: AgentRole::Delegated,
                available_tools: capabilities,
            },
            &config,
        )?;
        let initial = RunStatus {
            state: RunState::Starting,
            agent_id: Some(bound.id().to_string()),
            agent_fingerprint: Some(bound.fingerprint().to_string()),
            provider: Some(bound.config().provider.clone()),
            model: Some(bound.config().model.clone()),
            ..RunStatus::default()
        };
        subagent::write_status(&request.output_file, &initial)?;
        let (status_tx, status) = tokio::sync::watch::channel(initial);
        let (completion_tx, completion) = tokio::sync::watch::channel(false);
        let cancellation = RunCancellation::new();
        let task_cancellation = cancellation.clone();
        let config_path = self.config_path.clone();
        let cwd = self.cwd.clone();
        let permits = Arc::clone(&self.permits);
        let host_input = self.host_input.clone();
        let output_file = request.output_file;
        let parent_session_id = request.parent_session_id;
        let run_id = request.run_id;
        let persisted_output = output_file.clone();
        let prompt = request.prompt;

        let task_status_tx = status_tx.clone();
        let task: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            let Some(_permit) = acquire_permit_or_cancel(permits, &task_cancellation).await? else {
                let stopped = RunStatus {
                    state: RunState::Stopped,
                    agent_id: Some(bound.id().to_string()),
                    agent_fingerprint: Some(bound.fingerprint().to_string()),
                    provider: Some(bound.config().provider.clone()),
                    model: Some(bound.config().model.clone()),
                    last_activity: Some("cancelled before execution".into()),
                    ..RunStatus::default()
                };
                task_status_tx.send_replace(stopped.clone());
                subagent::write_status(&output_file, &stopped)?;
                return Ok(());
            };
            let mut config = bound.config().clone();
            super::cli_config::prepare_model_metadata(
                &config,
                &crate::credential_store::AppCredentialStore,
                &super::cli_config::ProviderRefreshStatus::NotAttempted,
            )
            .await;
            super::cli_config::normalize_reasoning(&mut config);
            let diagnostics = RuntimeDiagnostics::new(&config);
            diagnostics.update_agent(bound.id().as_str(), &bound.fingerprint().to_string());
            let mut reporter = RunReporter::new(
                output_file,
                RunArtifactIdentity {
                    agent_id: bound.id().to_string(),
                    agent_fingerprint: bound.fingerprint().to_string(),
                    provider: config.provider.clone(),
                    model: config.model.clone(),
                },
                cwd.clone(),
                &prompt,
                /* stream_output */ false,
                Some(task_status_tx),
            )?;
            let agent_id = bound.id().to_string();
            let startup = automation::Startup {
                config: &config,
                config_path,
                cwd,
                no_system_prompt: false,
                no_tools: false,
                no_subagents: true,
                usage_purpose: "subagent",
                parent_session_id: parent_session_id.clone(),
                agent: bound,
                output_file: None,
                output: OutputFormat::Text,
                max_steps: None,
                timeout: None,
                diagnostics,
                herdr: HerdrReporter::default(),
                host_input: questionnaire_available.then(|| automation::DelegatedHostInput {
                    run_id,
                    agent_id,
                    parent_session_id: parent_session_id
                        .expect("questionnaire bridge requires a parent session"),
                    bridge: host_input,
                }),
            };
            let result = automation::run_session(
                prompt,
                &startup,
                Some(&mut reporter),
                Some(task_cancellation),
            )
            .await;
            reporter.finish(&result);
            result.map(|_| ())
        });

        let failure_status = status.clone();
        tokio::spawn(async move {
            let failure = match task.await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(format!("delegated agent failed: {error:#}")),
                Err(error) if error.is_panic() => Some("delegated agent task panicked".into()),
                Err(error) => Some(format!("delegated agent task failed to join: {error}")),
            };
            if let Some(error) = failure {
                let mut failed = failure_status.borrow().clone();
                if !failed.state.is_terminal() {
                    failed.state = RunState::Error;
                    failed.error = Some(error);
                    status_tx.send_replace(failed.clone());
                    let _ = subagent::write_status(&persisted_output, &failed);
                }
            }
            completion_tx.send_replace(true);
        });

        Ok(AgentRunHandle {
            cancellation,
            status,
            completion,
        })
    }
}

async fn acquire_permit_or_cancel(
    permits: Arc<tokio::sync::Semaphore>,
    cancellation: &RunCancellation,
) -> anyhow::Result<Option<tokio::sync::OwnedSemaphorePermit>> {
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Ok(None),
        permit = permits.acquire_owned() => {
            let permit = permit.map_err(|_| {
                anyhow::anyhow!("agent executor shut down before the run could start")
            })?;
            if cancellation.is_cancelled() {
                Ok(None)
            } else {
                Ok(Some(permit))
            }
        }
    }
}

#[cfg(test)]
#[path = "agent_executor_tests.rs"]
mod tests;
