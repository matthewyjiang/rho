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
    agent_binding::{AgentBinder, AgentInvocation, AgentRole, CapacityClass},
    automation::{self, RunReporter},
    subagent_host_input::{SubagentHostInputBridge, SubagentHostInputResponder},
};

/// Default total concurrency across all delegated agents (Rho + Claude).
const DEFAULT_TOTAL_CONCURRENCY: usize = 4;
/// Default Claude-specific concurrency. Always nested under the total pool so
/// Claude fan-out cannot exceed this even when Rho capacity remains.
const DEFAULT_CLAUDE_CONCURRENCY: usize = 2;

#[derive(Clone)]
pub(crate) struct AgentExecutor {
    config: Arc<std::sync::RwLock<Config>>,
    config_path: PathBuf,
    cwd: PathBuf,
    /// Global permit pool for every delegated agent (Rho and Claude).
    ///
    /// `RHO_AGENT_CONCURRENCY` overrides this total. Claude runs also take a
    /// permit from [`Self::claude_permits`] (Claude pool first, then global) so
    /// one env override never opens a 2N fan-out window and queued Claude work
    /// cannot starve Rho of spare global capacity.
    /// `RHO_CLAUDE_AGENT_CONCURRENCY` overrides the Claude nested cap and is
    /// clamped to the total.
    total_permits: Arc<tokio::sync::Semaphore>,
    /// Claude-only nested pool. Sized to min(total, Claude default/override).
    /// Acquired before the global pool for Claude runs.
    claude_permits: Arc<tokio::sync::Semaphore>,
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

    #[cfg(test)]
    pub(crate) fn completed_for_test(status: RunStatus) -> Self {
        let (_status_tx, status_rx) = tokio::sync::watch::channel(status);
        let (_completion_tx, completion_rx) = tokio::sync::watch::channel(true);
        Self {
            cancellation: RunCancellation::new(),
            status: status_rx,
            completion: completion_rx,
        }
    }
}

impl AgentExecutor {
    pub(crate) fn new(
        config: Config,
        config_path: PathBuf,
        cwd: PathBuf,
        host_input: SubagentHostInputBridge,
    ) -> Self {
        let limits = concurrency_limits();
        Self {
            config: Arc::new(std::sync::RwLock::new(config)),
            config_path,
            cwd,
            total_permits: Arc::new(tokio::sync::Semaphore::new(limits.total)),
            claude_permits: Arc::new(tokio::sync::Semaphore::new(limits.claude)),
            host_input,
        }
    }

    pub(crate) fn host_input(&self) -> &SubagentHostInputBridge {
        &self.host_input
    }

    /// Atomically updates the parent session's provider selection inherited by
    /// future **Rho** runtime delegated agents.
    ///
    /// Claude-cli agents never read this snapshot for spawn: binding copies
    /// Claude model/tools/inherit from the definition only, byte-for-byte.
    pub(crate) fn update_selection(
        &self,
        provider: &str,
        model: &str,
        reasoning: rho_sdk::ReasoningLevel,
        auth: &str,
    ) {
        let mut config = self.config.write().expect("delegated config lock");
        config.provider = provider.to_string();
        config.model = model.to_string();
        config.reasoning = reasoning;
        config.auth = auth.to_string();
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

        let labels = bound.runtime().artifact_labels();
        let capacity_class = bound.runtime().capacity_class();

        let initial = RunStatus {
            state: RunState::Starting,
            agent_id: Some(bound.id().to_string()),
            agent_fingerprint: Some(bound.fingerprint().to_string()),
            provider: Some(labels.provider.clone()),
            model: Some(labels.model.clone()),
            parent_session_id: request.parent_session_id.as_ref().map(ToString::to_string),
            ..RunStatus::default()
        };
        // Executor owns the Starting boundary; sinks continue_from it.
        // Write Starting here so the handle can observe status before the task runs.
        subagent::initialize_status(&request.output_file, &initial)?;
        let (status_tx, status) = tokio::sync::watch::channel(initial);
        let (completion_tx, completion) = tokio::sync::watch::channel(false);
        let cancellation = RunCancellation::new();
        let task_cancellation = cancellation.clone();
        let config_path = self.config_path.clone();
        let cwd = self.cwd.clone();
        let host_input = self.host_input.clone();
        let output_file = request.output_file;
        let parent_session_id = request.parent_session_id;
        let run_id = request.run_id;
        let persisted_output = output_file.clone();
        let prompt = request.prompt;
        let total_permits = Arc::clone(&self.total_permits);
        let claude_permits = Arc::clone(&self.claude_permits);

        let task_status_tx = status_tx.clone();
        let task: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            // Acquire runtime-aware capacity before work starts. Claude runs
            // take Claude capacity first so queued Claude work cannot occupy
            // global permits while waiting on the nested pool; Rho takes only
            // the global pool. Dropping the guard returns every held permit.
            let Some(_runtime_permits) = acquire_runtime_permits(
                total_permits,
                claude_permits,
                capacity_class,
                &task_cancellation,
            )
            .await?
            else {
                let mut stopped = task_status_tx.borrow().clone();
                stopped.state = RunState::Stopped;
                stopped.last_activity = Some("cancelled before execution".into());
                task_status_tx.send_replace(stopped.clone());
                subagent::write_status(&output_file, &stopped)?;
                return Ok(());
            };

            let started_status = task_status_tx.borrow().clone();
            if let Some(session) = bound.clone().into_claude_session(
                prompt.clone(),
                output_file.clone(),
                cwd.clone(),
                task_cancellation.clone(),
                Some(task_status_tx.clone()),
                Some(started_status),
            ) {
                return crate::claude_runtime::session::run_session(session).await;
            }

            let bound_config = bound
                .rho_config()
                .expect("non-Claude bound runtime is Rho")
                .clone();
            run_rho_agent(RhoAgentRun {
                bound,
                config: bound_config,
                config_path,
                cwd,
                prompt,
                output_file,
                run_id,
                parent_session_id,
                questionnaire_available,
                host_input,
                cancellation: task_cancellation,
                status_tx: task_status_tx,
            })
            .await
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

/// One delegated run on the Rho runtime, after binding and permit acquisition.
struct RhoAgentRun {
    bound: super::agent_binding::BoundAgent,
    config: Config,
    config_path: PathBuf,
    cwd: PathBuf,
    prompt: String,
    output_file: PathBuf,
    run_id: String,
    parent_session_id: Option<rho_sdk::SessionId>,
    questionnaire_available: bool,
    host_input: SubagentHostInputBridge,
    cancellation: RunCancellation,
    status_tx: tokio::sync::watch::Sender<RunStatus>,
}

/// Drive a delegated run through Rho's own automation loop.
async fn run_rho_agent(run: RhoAgentRun) -> anyhow::Result<()> {
    let RhoAgentRun {
        bound,
        mut config,
        config_path,
        cwd,
        prompt,
        output_file,
        run_id,
        parent_session_id,
        questionnaire_available,
        host_input,
        cancellation,
        status_tx,
    } = run;

    super::cli_config::prepare_model_metadata(
        &config,
        &crate::credential_store::AppCredentialStore,
        &super::cli_config::ProviderRefreshStatus::NotAttempted,
    )
    .await;
    super::cli_config::normalize_reasoning(&mut config);
    let diagnostics = RuntimeDiagnostics::new(&config);
    diagnostics.update_agent(bound.id().as_str(), &bound.fingerprint().to_string());
    let started_status = status_tx.borrow().clone();
    let mut reporter = RunReporter::continue_from(
        output_file,
        started_status,
        cwd.clone(),
        &prompt,
        /* stream_output */ false,
        Some(status_tx),
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
        host_input: questionnaire_available.then(|| {
            Arc::new(SubagentHostInputResponder::new(
                run_id,
                agent_id,
                parent_session_id.expect("questionnaire bridge requires a parent session"),
                host_input,
            )) as Arc<dyn super::headless_run::HostInputResponder>
        }),
    };
    let result =
        automation::run_session(prompt, &startup, Some(&mut reporter), Some(cancellation)).await;
    reporter.finish(&result);
    result.map(|_| ())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ConcurrencyLimits {
    total: usize,
    claude: usize,
}

fn concurrency_limits() -> ConcurrencyLimits {
    concurrency_limits_from_env(
        std::env::var("RHO_AGENT_CONCURRENCY").ok().as_deref(),
        std::env::var("RHO_CLAUDE_AGENT_CONCURRENCY")
            .ok()
            .as_deref(),
    )
}

/// Parse concurrency env values without reading process environment.
///
/// Both values must be positive `usize` integers when present. Zero, empty,
/// non-numeric, and overflow values fall back to the matching default. The
/// Claude nested cap is always clamped to the resolved total so Claude fan-out
/// cannot exceed the global pool.
fn concurrency_limits_from_env(
    total_raw: Option<&str>,
    claude_raw: Option<&str>,
) -> ConcurrencyLimits {
    let total = parse_positive_concurrency(total_raw).unwrap_or(DEFAULT_TOTAL_CONCURRENCY);
    let claude_requested =
        parse_positive_concurrency(claude_raw).unwrap_or(DEFAULT_CLAUDE_CONCURRENCY);
    // Nested Claude pool never exceeds the global total, so overrides cannot
    // open total + Claude = 2N concurrent delegated runs.
    let claude = claude_requested.min(total);
    ConcurrencyLimits { total, claude }
}

fn parse_positive_concurrency(raw: Option<&str>) -> Option<usize> {
    raw.and_then(|value| value.parse().ok())
        .filter(|limit: &usize| *limit > 0)
}

/// Concurrency capacity held for the lifetime of one delegated run.
///
/// Dropping this value returns every acquired permit. Field drop order is not
/// load-bearing; each permit returns to its own semaphore independently.
struct RuntimePermits {
    _total: tokio::sync::OwnedSemaphorePermit,
    _claude: Option<tokio::sync::OwnedSemaphorePermit>,
}

/// Acquire concurrency for a delegated run in runtime-aware order.
///
/// - Rho: one global permit only.
/// - Claude: Claude nested permit first, then one global permit.
///
/// Claude-first ordering keeps queued Claude tasks off the global pool until
/// Claude capacity exists, so spare global slots stay available for Rho.
/// Cancellation at either wait stage returns `Ok(None)` and drops any permit
/// already held so capacity cannot leak. A closed semaphore surfaces as an
/// error rather than a silent cancel.
async fn acquire_runtime_permits(
    total_permits: Arc<tokio::sync::Semaphore>,
    claude_permits: Arc<tokio::sync::Semaphore>,
    capacity_class: CapacityClass,
    cancellation: &RunCancellation,
) -> anyhow::Result<Option<RuntimePermits>> {
    let claude = match capacity_class {
        CapacityClass::Claude => {
            let Some(permit) = acquire_permit_or_cancel(
                claude_permits,
                cancellation,
                "Claude agent concurrency pool closed before the run could start",
            )
            .await?
            else {
                return Ok(None);
            };
            Some(permit)
        }
        CapacityClass::Rho => None,
    };

    let Some(total) = acquire_permit_or_cancel(
        total_permits,
        cancellation,
        "agent concurrency pool closed before the run could start",
    )
    .await?
    else {
        // Drop any Claude permit acquired above before returning cancelled.
        return Ok(None);
    };

    Ok(Some(RuntimePermits {
        _total: total,
        _claude: claude,
    }))
}

async fn acquire_permit_or_cancel(
    permits: Arc<tokio::sync::Semaphore>,
    cancellation: &RunCancellation,
    closed_message: &str,
) -> anyhow::Result<Option<tokio::sync::OwnedSemaphorePermit>> {
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Ok(None),
        permit = permits.acquire_owned() => {
            // Map a closed semaphore to a clear startup error. Dropping the
            // failed acquire path holds nothing, so no permit can leak here.
            let permit = permit.map_err(|_| anyhow::anyhow!("{closed_message}"))?;
            // Re-check after acquire wins the select so a cancel that arrived
            // in the same poll drops the permit via this None return.
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
