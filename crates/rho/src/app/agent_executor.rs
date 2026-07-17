use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use crate::{
    agent::{AgentDefinition, KNOWN_TOOLS},
    cancellation::RunCancellation,
    config::Config,
    diagnostics::RuntimeDiagnostics,
    herdr::HerdrReporter,
    subagent::{self, RunState, RunStatus},
};

use super::{
    agent_binding::{AgentBinder, AgentInvocation, AgentRole},
    automation::{self, RunArtifactIdentity, RunReporter},
};

#[derive(Clone)]
pub(crate) struct AgentExecutor {
    config: Arc<std::sync::RwLock<Config>>,
    config_path: PathBuf,
    cwd: PathBuf,
    permits: Arc<tokio::sync::Semaphore>,
}

pub(crate) struct AgentLaunchRequest {
    pub(crate) definition: Arc<AgentDefinition>,
    pub(crate) prompt: String,
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
    pub(crate) fn new(config: Config, config_path: PathBuf, cwd: PathBuf) -> Self {
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
        }
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

    pub(crate) fn spawn(&self, request: AgentLaunchRequest) -> anyhow::Result<AgentRunHandle> {
        let config = self.config.read().expect("delegated config lock").clone();
        let mut capabilities = KNOWN_TOOLS
            .iter()
            .map(|tool| (*tool).to_string())
            .collect::<BTreeSet<_>>();
        if !crate::tools::web::access_tools(&config).is_available() {
            capabilities.remove("web_search");
        }
        #[cfg(windows)]
        capabilities.remove("bash");
        #[cfg(not(windows))]
        capabilities.remove("powershell");
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
        let output_file = request.output_file;
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
            if config.provider == "anthropic"
                && crate::model::models_dev::cached_model_metadata(&config.provider, &config.model)
                    .is_none()
            {
                let _ =
                    crate::model::models_dev::fetch_model_metadata(&config.provider, &config.model)
                        .await;
            }
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
            let startup = automation::Startup {
                config: &config,
                config_path,
                cwd,
                no_system_prompt: false,
                no_tools: false,
                no_subagents: true,
                agent: bound,
                output_file: None,
                diagnostics,
                herdr: HerdrReporter::default(),
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
