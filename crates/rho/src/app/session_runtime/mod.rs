//! The UI-free session runtime: one per thread, owning a built session, its
//! file, and the state between prompts.
//!
//! A host opens it with [`SessionRuntime::open_new`] or
//! [`SessionRuntime::reopen`] (see `open.rs`), drives it, and shows what it
//! reports, holding it exclusively for each prompt or setting change. Nothing here knows a wire protocol or a
//! client: hosts parse their own requests and map the runtime's results,
//! events, and errors back to theirs.
//!
//! Between prompts a host calls the runtime's settings cores directly:
//! permission mode, model and provider switches, reasoning level, and fast
//! mode. Hosts read the session only through narrow accessors, never the
//! SDK session itself, so all changes go through the runtime. A prompt starts
//! with [`SessionRuntime::begin_prompt`] (see `prompt.rs`), whose
//! [`PromptScope`] every in-prompt call takes, so only a begun prompt runs
//! the command cores (`/compact`, `/workflow:`, `/agent:` rebinds) or a
//! model turn. A host runs a prompt's model turn through
//! [`SessionRuntime::run_prompt_turn`] (see `prompt_turn.rs`) with a
//! [`TurnView`], which opens each turn's [`TurnIo`] and, while the turn runs,
//! shows the [`SessionEvent`]s it sends and answers its requests with
//! [`TurnControl`]s (see `events.rs`). The turn saves itself, runs the
//! session hooks, keeps background work going across prompts, and asks to
//! name the session; `run_prompt_turn` then hands off a plan the turn
//! answered and runs the implementation turn it may ask for. The host maps
//! the prompt's end to its own reply.
//!
//! A prompt's cancel goes through the runtime's [`SessionHandle`], which the
//! host shares with whoever cancels prompts or retires the session, so a
//! cancel that arrives while a prompt is still starting cancels its run once
//! it exists.

use std::path::Path;

use rho_providers::model::models_dev;

use crate::{
    added_dirs::AddedDirs,
    app::{
        agent_binding::BoundAgent, headless_delegation::HeldNotices, session_assembly::BuiltSession,
    },
    config::Config,
    permission::PermissionMode,
    tools::mcp::config::McpConfig,
};

mod agent;
mod approval_policy;
mod build;
mod compact;
mod credentials;
mod events;
mod handle;
mod mode;
mod open;
mod persist;
mod prompt;
mod prompt_gate;
mod prompt_turn;
mod pump;
mod session_tools;
mod settings;
mod switch;
#[cfg(test)]
mod test_support;
mod title;
mod turn;
mod workflow;

pub(crate) use agent::AgentTask;
#[cfg(test)]
pub(in crate::app) use agent::{bind_delegated_agent, loaded_agent_with, MODEL_OVERRIDE_METADATA};
pub(crate) use approval_policy::ApprovalPolicy;
#[cfg(test)]
pub(in crate::app) use build::session_config;
pub(crate) use build::{ChildAgents, QuestionSupport, RunShape, SessionBuild, SessionRole};
use credentials::SessionCredentials;
pub(crate) use credentials::{CredentialFailure, Logouts};
pub(crate) use events::{
    cancelled_approval, ApprovalId, InsertedMessage, PlanApprovalId, PlanApprovalRequest, RunEnd,
    SessionEvent, TurnControl,
};
pub(crate) use handle::SessionHandle;
pub(crate) use mode::SessionPermission;
pub(crate) use open::{SavedHistory, SessionOpening};
pub(crate) use persist::SessionStorage;
pub(crate) use prompt::PromptScope;
pub(crate) use prompt_gate::Closing;
pub(crate) use prompt_turn::{PlanReport, TurnView};
pub(crate) use session_tools::SessionTools;
pub(crate) use settings::SettingChangeError;
pub(crate) use title::TitleJob;
use title::TitleState;
pub(crate) use turn::{Turn, TurnIo, TurnOutcome};
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

/// A session opened new or reopened (see `open.rs`), and the state it
/// starts from.
struct OpenedSession {
    /// How the session was built, kept so an agent rebind builds the same
    /// way.
    build: SessionBuild,
    built: BuiltSession,
    storage: SessionStorage,
    /// The agent the session runs as: the launch agent, or for a reopened
    /// child the agent it was saved as.
    agent: BoundAgent,
    /// The auth profile the session's provider uses, which a reopened child
    /// may take from its saved selection.
    auth: String,
    /// The host's own MCP servers, overlaid on the process config whenever
    /// the session is built again.
    host_mcp: McpConfig,
    /// Launch `--add-dir` values plus the host's own roots.
    added_dirs: AddedDirs,
    permission: SessionPermission,
    /// A notice found while reopening, such as a changed model prompt,
    /// reported with the first turn.
    reopen_notice: Option<String>,
}

/// One session's runtime: everything a host keeps for a session except how
/// it talks to its client.
pub(crate) struct SessionRuntime {
    built: BuiltSession,
    storage: SessionStorage,
    agent: BoundAgent,
    build: SessionBuild,
    host_mcp: McpConfig,
    /// Saved with every snapshot so a TUI resume restores them; a later
    /// reload takes only the roots its request names.
    added_dirs: AddedDirs,
    permission: SessionPermission,
    auth: String,
    credentials: SessionCredentials,
    /// Cancels this runtime's prompts and marks it replaced; see
    /// [`Self::handle`].
    handle: SessionHandle,
    completed_runs: u64,
    /// How far this runtime got asking to name the session.
    title: TitleState,
    /// Set when a compaction rewrote live state but could not be saved.
    /// Later prompts are refused so the session never diverges further
    /// from its file; a reload starts a fresh runtime from storage.
    unsaved_compaction: bool,
    /// A notice found while reopening, reported with the first turn, because
    /// hosts such as T3 Code discard live updates while a load is in flight.
    reopen_notice: Option<String>,
    /// Child notice binding kept between prompts, since children outlive them.
    held_notices: Option<HeldNotices>,
}

impl SessionRuntime {
    /// A runtime for `opened`. `logouts` is the host's logout count; the
    /// session's provider has just read credentials.
    fn new(opened: OpenedSession, logouts: &Logouts) -> Self {
        let OpenedSession {
            build,
            built,
            storage,
            agent,
            auth,
            host_mcp,
            added_dirs,
            permission,
            reopen_notice,
        } = opened;
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
            credentials: SessionCredentials::just_read(logouts),
            handle: SessionHandle::new(),
            completed_runs: 0,
            title: TitleState::Unasked,
            unsaved_compaction: false,
            reopen_notice,
            held_notices: None,
        }
    }

    pub(crate) fn session_id(&self) -> &rho_sdk::SessionId {
        self.built.session.id()
    }

    /// A read-only view of the session's tools; see [`SessionTools`].
    pub(crate) fn tools(&self) -> SessionTools<'_> {
        SessionTools::of(&self.built.tools)
    }

    /// Reads the session's context fill against its current model's
    /// window, including while a turn runs. Take a new gauge after a model
    /// switch.
    pub(crate) fn context_gauge(&self) -> ContextGauge {
        let current = self.current_model();
        ContextGauge {
            session: self.built.session.clone(),
            configured_window: model_context_window(&current.provider, &current.model),
        }
    }

    /// The session's workspace directory.
    pub(crate) fn cwd(&self) -> &Path {
        self.storage.cwd()
    }

    pub(crate) fn permission_mode(&self) -> PermissionMode {
        self.permission.mode
    }

    /// The handle that cancels this runtime's prompts, retires it, and
    /// hands its file to a successor. Hosts that publish a session share it,
    /// so they can tell this runtime from a successor.
    pub(crate) fn handle(&self) -> SessionHandle {
        self.handle.clone()
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

    /// Cancels any prompt and tears the session down.
    pub(crate) async fn shutdown(self) {
        self.handle.cancel();
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

/// Reads one session's context fill and the window it fills, and nothing
/// that changes the session. A host keeps it while a turn runs, when the
/// runtime itself is busy.
#[derive(Clone)]
pub(crate) struct ContextGauge {
    session: rho_sdk::Session,
    /// The configured window of the session's model when the gauge was
    /// taken.
    configured_window: Option<u64>,
}

impl ContextGauge {
    /// A gauge on `session` whose model is configured with
    /// `configured_window`, for host tests that have no runtime.
    #[cfg(test)]
    pub(in crate::app) fn of_session_for_tests(
        session: rho_sdk::Session,
        configured_window: Option<u64>,
    ) -> Self {
        Self {
            session,
            configured_window,
        }
    }

    pub(crate) fn estimate(&self) -> rho_sdk::ContextEstimate {
        self.session.context_estimate()
    }

    /// The context window `estimate` fills: the configured window of the
    /// session's model, else the one the provider reported with `estimate`,
    /// as the TUI context meter measures it. `None` when neither knows.
    pub(crate) fn window_for(&self, estimate: rho_sdk::ContextEstimate) -> Option<u64> {
        self.configured_window
            .or(estimate.reported_context_window())
    }

    pub(crate) fn session_id(&self) -> &rho_sdk::SessionId {
        self.session.id()
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
