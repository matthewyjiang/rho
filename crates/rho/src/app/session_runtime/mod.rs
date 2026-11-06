//! The UI-free session runtime: one per thread, owning a built session, its
//! file, and the state between prompts.
//!
//! A host drives it and shows what it reports. ACP (`app::acp`) is the host
//! today; the daemon and `rho run` host it next. Nothing here knows a wire
//! protocol: hosts parse their own requests and map the runtime's results and
//! errors back to theirs.
//!
//! Between prompts a host calls the runtime's settings and command cores
//! directly: permission mode, model and provider switches, `/compact`,
//! `/workflow:`, `/agent:` rebinds, and the session title. A model turn is
//! [`SessionRuntime::run_turn`]: the host passes a [`TurnIo`] and, while the
//! turn runs, shows the [`SessionEvent`]s it sends and answers its requests
//! with [`TurnControl`]s (see `events.rs`). The turn saves itself, runs the
//! session hooks, and keeps background work going across prompts; the host
//! only maps the [`TurnOutcome`] to its own reply.
//!
//! A prompt's cancel goes through the [`PromptGate`], which the host may
//! share with whoever cancels prompts, so a cancel that arrives while a
//! prompt is still starting cancels its run once it exists.

use std::{
    path::Path,
    sync::{atomic::AtomicBool, Arc},
};

use rho_providers::model::models_dev;

use crate::{
    added_dirs::AddedDirs,
    app::{
        agent_binding::BoundAgent,
        headless_delegation::{BackgroundWait, HeldNotices},
        session_assembly::{BuiltSession, SessionAssembly},
    },
    config::Config,
    permission::PermissionMode,
    tools::mcp::config::McpConfig,
};

mod agent;
mod build;
mod compact;
mod credentials;
mod events;
mod mode;
mod persist;
mod prompt_gate;
mod pump;
mod switch;
mod title;
mod turn;
mod workflow;

#[cfg(test)]
pub(crate) use agent::{bind_delegated_agent, loaded_agent_with, MODEL_OVERRIDE_METADATA};
pub(crate) use agent::{loaded_agent, AgentTask};
#[cfg(test)]
pub(crate) use build::overlay_host_mcp;
pub(crate) use build::{build_session, session_config, ChildAgents, QuestionSupport, SessionBuild};
use credentials::SessionCredentials;
pub(crate) use credentials::{CredentialFailure, Logouts};
pub(crate) use events::{
    cancelled_approval, ApprovalId, InsertedMessage, PlanApprovalId, PlanApprovalRequest, RunEnd,
    SessionEvent, TurnControl,
};
pub(crate) use mode::{ModeChangeError, SessionPermission};
pub(crate) use persist::HostStorage;
pub(crate) use prompt_gate::{Closing, PromptGate, PromptGuard};
pub(crate) use switch::ModelSwitchError;
pub(crate) use title::TitleJob;
pub(crate) use turn::{MessageSave, Turn, TurnIo, TurnOutcome};
pub(crate) use workflow::WorkflowStart;

/// A request the session recognizes but cannot run, such as a TUI-only
/// command, an MCP prompt missing arguments, or `/agent:` on a session that
/// already has history. The message is user-facing.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct CommandRejected(pub(crate) String);

/// The model and auth profile a session runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CurrentModel {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) auth: String,
}

/// What a host supplies to start a runtime around a built session.
pub(crate) struct RuntimeParts {
    pub(crate) built: BuiltSession,
    pub(crate) storage: HostStorage,
    pub(crate) agent: BoundAgent,
    pub(crate) build: SessionBuild,
    /// The host's own MCP servers, overlaid on the process config whenever
    /// the session is built again.
    pub(crate) host_mcp: McpConfig,
    /// Launch `--add-dir` values plus the host's own roots.
    pub(crate) added_dirs: AddedDirs,
    pub(crate) permission: SessionPermission,
    /// The auth profile the session's provider uses.
    pub(crate) auth: String,
    /// The host's logout count. The provider has just read credentials.
    pub(crate) logouts: Logouts,
    /// Whether background work holds a prompt open or the host wakes the
    /// idle session when it finishes.
    pub(crate) background_wait: BackgroundWait,
}

/// One session's runtime: everything a host keeps for a session except how
/// it talks to its client.
pub(crate) struct SessionRuntime {
    built: BuiltSession,
    storage: HostStorage,
    agent: BoundAgent,
    build: SessionBuild,
    host_mcp: McpConfig,
    /// Saved with every snapshot so a TUI resume restores them; a later
    /// reload takes only the roots its request names.
    added_dirs: AddedDirs,
    permission: SessionPermission,
    auth: String,
    credentials: SessionCredentials,
    prompt_gate: Arc<PromptGate>,
    completed_runs: u64,
    /// Whether this runtime asked the title agent to name the session.
    title_requested: bool,
    /// The title the last prompt asked for, until the host takes it.
    pending_title: Option<TitleJob>,
    /// Set once a successor owns the session file; see
    /// [`Self::replaced_flag`].
    replaced: Arc<AtomicBool>,
    /// Set when a compaction rewrote live state but could not be saved.
    /// Later prompts are refused so the session never diverges further
    /// from its file; a reload starts a fresh runtime from storage.
    unsaved_compaction: bool,
    /// A notice found while reopening, reported with the first turn, because
    /// hosts such as T3 Code discard live updates while a load is in flight.
    deferred_notice: Option<String>,
    /// Child notice binding kept between prompts, since children outlive them.
    held_notices: Option<HeldNotices>,
    background_wait: BackgroundWait,
}

impl SessionRuntime {
    pub(crate) fn new(parts: RuntimeParts) -> Self {
        let RuntimeParts {
            built,
            storage,
            agent,
            build,
            host_mcp,
            added_dirs,
            permission,
            auth,
            logouts,
            background_wait,
        } = parts;
        built.tools.bind_recall(storage.file());
        Self {
            built,
            storage,
            agent,
            build,
            host_mcp,
            added_dirs,
            permission,
            auth,
            credentials: SessionCredentials::just_read(&logouts),
            prompt_gate: Arc::new(PromptGate::new()),
            completed_runs: 0,
            title_requested: false,
            pending_title: None,
            replaced: Arc::new(AtomicBool::new(false)),
            unsaved_compaction: false,
            deferred_notice: None,
            held_notices: None,
            background_wait,
        }
    }

    /// Holds `notice` for the next turn to report before its run starts.
    pub(crate) fn defer_notice(&mut self, notice: String) {
        self.deferred_notice = Some(notice);
    }

    /// Runs a reopened session as the `agent` it was saved with, on its
    /// `auth` profile: a restored child may differ from the launch agent in
    /// both.
    pub(crate) fn restore_agent(&mut self, agent: BoundAgent, auth: String) {
        self.agent = agent;
        self.auth = auth;
    }

    pub(crate) fn built(&self) -> &BuiltSession {
        &self.built
    }

    /// The session's workspace directory.
    pub(crate) fn cwd(&self) -> &Path {
        self.storage.cwd()
    }

    pub(crate) fn permission_mode(&self) -> PermissionMode {
        self.permission.mode
    }

    /// The gate that cancels this session's prompts. Hosts that publish a
    /// session share it, so they can tell this runtime from a successor.
    pub(crate) fn cancel_handle(&self) -> Arc<PromptGate> {
        Arc::clone(&self.prompt_gate)
    }

    pub(crate) fn prompt_gate(&self) -> &PromptGate {
        &self.prompt_gate
    }

    /// Set it once another runtime owns the session file, so this one no
    /// longer writes it.
    pub(crate) fn replaced_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.replaced)
    }

    /// Fails when a compaction rewrote live state that could not be saved,
    /// which leaves the session refusing prompts until it is reloaded.
    pub(crate) fn ensure_saved_state(&self) -> anyhow::Result<()> {
        if self.unsaved_compaction {
            anyhow::bail!(
                "could not continue session: its compacted context was not saved; load the session again"
            );
        }
        Ok(())
    }

    /// The answer to `exit_plan_mode` that the last turn recorded, if any.
    pub(crate) fn take_plan_exit_decision(
        &self,
    ) -> Option<crate::tools::plan_exit::PlanExitDecision> {
        self.built.tools.take_plan_exit_decision()
    }

    pub(crate) fn current_model(&self) -> CurrentModel {
        let snapshot = self.built.session.snapshot();
        let identity = snapshot.provider();
        CurrentModel {
            provider: identity.provider.clone(),
            model: identity.model.clone(),
            auth: self.auth.clone(),
        }
    }

    /// The configured context window of the current model.
    pub(crate) fn context_window(&self) -> Option<u64> {
        let current = self.current_model();
        model_context_window(&current.provider, &current.model)
    }

    /// Cancels any prompt and tears the session down.
    pub(crate) async fn shutdown(self) {
        self.prompt_gate.cancel();
        self.built.teardown().await;
    }

    fn dispatch_failed(&self, message: &str) {
        self.built.runtime.hooks().session_failed(
            self.built.session.id(),
            rho_sdk::hooks::HookSessionFailureKind::RunFailed,
            message,
        );
    }
}

/// `process_config` with the session's current model and auth.
pub(crate) fn model_scoped_config(current: &CurrentModel, process_config: &Config) -> Config {
    let mut config = process_config.clone();
    config.provider = current.provider.clone();
    config.model = current.model.clone();
    config.auth = current.auth.clone();
    config
}

fn model_context_window(provider: &str, model: &str) -> Option<u64> {
    models_dev::cached_model_metadata(provider, model)
        .and_then(|metadata| metadata.display_context_window())
}

/// Splits an assembly into the built session and the permission state for
/// `mode`.
pub(crate) fn split_assembly(
    assembly: SessionAssembly,
    mode: PermissionMode,
) -> (BuiltSession, SessionPermission) {
    let SessionAssembly {
        built,
        workspace_root,
        session_writes,
    } = assembly;
    (
        built,
        SessionPermission {
            mode,
            writes: session_writes,
            workspace_root,
        },
    )
}

#[cfg(test)]
impl SessionRuntime {
    /// A runtime around `built`, saved through `storage`, for a plain root
    /// agent with every tool whose workspace is the storage's.
    pub(crate) fn for_tests(built: BuiltSession, storage: HostStorage, config: &Config) -> Self {
        use crate::{
            agent::{
                AgentCapabilities, AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy,
                PromptPolicy, ToolPolicy,
            },
            app::agent_binding::{AgentBinder, AgentInvocation, AgentRole},
        };
        let agent = AgentBinder::bind(
            Arc::new(AgentDefinition {
                id: AgentId::new("rho").expect("test agent id"),
                description: "test".into(),
                prompt: PromptPolicy::Extend(String::new()),
                runtime: AgentRuntimeSpec::Rho {
                    tools: ToolPolicy::All,
                    model: ModelPolicy::Inherit,
                    reasoning: None,
                },
            }),
            AgentInvocation {
                role: AgentRole::AutomationRoot,
                available_tools: AgentCapabilities::all_host_tools(),
            },
            config,
        )
        .expect("bind test agent");
        let workspace_root = storage.cwd().to_path_buf();
        Self::new(RuntimeParts {
            built,
            storage,
            agent: agent.clone(),
            build: SessionBuild {
                config_path: workspace_root.join("rho-test-config.toml"),
                no_system_prompt: false,
                no_tools: false,
                no_subagents: false,
                diagnostics: crate::diagnostics::RuntimeDiagnostics::new(config),
                questions: QuestionSupport::Unavailable,
                children: ChildAgents::InProcess,
                launch_agent: agent,
                host_io: Default::default(),
            },
            host_mcp: McpConfig::default(),
            added_dirs: AddedDirs::default(),
            permission: SessionPermission {
                mode: config.permission_mode,
                writes: Default::default(),
                workspace_root,
            },
            auth: config.auth.clone(),
            logouts: Logouts::default(),
            background_wait: BackgroundWait::HoldPrompt,
        })
    }

    /// Pretends the runtime already completed `runs` turns.
    pub(crate) fn set_completed_runs_for_tests(&mut self, runs: u64) {
        self.completed_runs = runs;
    }

    pub(crate) fn credentials_for_tests(&self) -> &SessionCredentials {
        &self.credentials
    }

    pub(crate) fn storage(&self) -> &HostStorage {
        &self.storage
    }

    pub(crate) fn agent(&self) -> &BoundAgent {
        &self.agent
    }
}
