//! The UI-free session runtime: one per thread, owning a built session, its
//! file, and the state between prompts.
//!
//! A host opens it with [`SessionRuntime::open_new`] or
//! [`SessionRuntime::open_saved`] (see `open.rs`), drives it, and shows what
//! it reports, holding it exclusively for each prompt or setting change.
//! Nothing here knows a wire protocol or a client: hosts parse their own
//! requests and map the runtime's results, events, and errors back to
//! theirs.
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
//! A prompt's cancel and steering go through the runtime's
//! [`SessionHandle`], which the host shares with whoever cancels or steers
//! prompts or closes the session, so a cancel that arrives while a prompt
//! is still starting cancels its run once it exists.

use std::path::Path;

use rho_providers::model::models_dev;

use crate::{
    added_dirs::AddedDirs,
    app::{agent_binding::BoundAgent, session_assembly::BuiltSession},
    config::Config,
    permission::PermissionMode,
    tools::mcp::config::McpConfig,
};

mod added_dirs;
mod advisor;
mod agent;
pub(in crate::app) mod agent_catalog;
mod approval_policy;
mod build;
mod codemode;
mod compact;
mod context;
mod credentials;
mod delegation;
mod events;
mod handle;
mod inspect;
mod left_out;
mod mode;
mod open;
mod persist;
mod prompt;
mod prompt_gate;
mod prompt_turn;
mod pump;
mod saved_settings;
mod session_tools;
mod settings;
mod switch;
#[cfg(test)]
mod test_support;
mod title;
mod tool_sources;
mod tree;
mod turn;
mod workflow;
mod workspace_rewind;

pub(crate) use added_dirs::AddDirError;
pub(crate) use agent::AgentTask;
pub(crate) use approval_policy::{
    ensure_auto_classifier_model, ApprovalPolicy, UnattendedApprovals,
};
#[cfg(test)]
pub(in crate::app) use build::session_config;
pub(crate) use build::{
    ChildAgents, HostCapabilities, IdleTurns, QuestionSupport, RunShape, SessionBuild,
    SessionLaunch, SessionRole, SessionUse,
};
pub(crate) use credentials::CredentialFailure;
use credentials::SessionCredentials;
pub(crate) use delegation::BackgroundWait;
use delegation::HeldNotices;
pub(crate) use events::{
    cancelled_approval, ApprovalId, InsertedMessage, PlanApprovalId, PlanApprovalRequest,
    SessionEvent, TurnControl,
};
pub(crate) use handle::{SessionHandle, SteerError};
pub(crate) use left_out::LeftOut;
pub(crate) use mode::SessionPermission;
pub(crate) use open::{NodeSession, SessionOpening};
pub(crate) use persist::SessionStorage;
pub(crate) use prompt::PromptScope;
pub(crate) use prompt_turn::{PlanReport, TurnView};
use saved_settings::SettingsPolicy;
pub(crate) use session_tools::SessionTools;
pub(crate) use settings::SettingChangeError;
#[cfg(test)]
pub(in crate::app) use test_support::{
    unattended_runtime_on, unsaved_unattended_runtime_on, HangUntilCancel,
};
pub(crate) use title::TitleJob;
use title::TitleState;
pub(crate) use tree::SelectNodeError;
pub(crate) use turn::{Turn, TurnIo, TurnOutcome, TurnStart};
pub(crate) use workflow::WorkflowStart;
pub(crate) use workspace_rewind::{RewindError, RewoundConversation};

/// A request the session recognizes but cannot run, such as a TUI-only
/// command, an MCP prompt missing arguments, or `/agent:` on a session that
/// already has history. The message is user-facing.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct CommandRejected(pub(crate) String);

/// [`SessionRuntime::begin_prompt`]'s refusal after a compaction rewrote
/// live state it could not save. Hosts that reload sessions differently
/// say how in their own words.
#[derive(Debug, thiserror::Error)]
#[error("could not continue session: its compacted context was not saved; load the session again")]
pub(crate) struct UnsavedCompaction;

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
    /// What the session's provider read its credentials at (#1662).
    credentials: SessionCredentials,
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
    /// Cancels this runtime's prompts; see [`Self::handle`].
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
    /// Where the questions of the session's subagents go when the host
    /// takes them itself (see [`Self::host_takes_child_questions`]);
    /// `None` while each prompt takes them for its own run.
    child_questions: Option<
        tokio::sync::mpsc::Sender<crate::app::subagent_host_input::SubagentHostInputRequest>,
    >,
    /// Whether the session's saves record its settings for a reopen.
    settings: SettingsPolicy,
    /// Set once checkpoint storage failed or filled: later turns take no
    /// workspace checkpoint.
    checkpoints_paused: bool,
}

impl SessionRuntime {
    /// A runtime for `opened`.
    fn new(opened: OpenedSession) -> Self {
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
            credentials,
        } = opened;
        built.tools.bind_recall(storage.file());
        let settings = SettingsPolicy::of(storage.file());
        Self {
            built,
            storage,
            agent,
            build,
            host_mcp,
            added_dirs,
            permission,
            auth,
            credentials,
            handle: SessionHandle::new(),
            completed_runs: 0,
            title: TitleState::Unasked,
            unsaved_compaction: false,
            reopen_notice,
            held_notices: None,
            child_questions: None,
            settings,
            checkpoints_paused: false,
        }
    }

    pub(crate) fn session_id(&self) -> &rho_sdk::SessionId {
        self.built.session.id()
    }

    /// The session's credentials: those of the environment it opened with
    /// (see [`crate::session_environment::SessionEnvironment::credentials`]).
    #[cfg_attr(not(unix), allow(dead_code, reason = "only the Unix daemon asks"))]
    pub(crate) fn credentials(
        &self,
    ) -> std::sync::Arc<dyn rho_providers::credentials::CredentialStore> {
        self.build.host.environment.credentials()
    }

    /// A read-only view of the session's tools; see [`SessionTools`].
    pub(crate) fn tools(&self) -> SessionTools<'_> {
        SessionTools::of(&self.built.tools)
    }

    /// The configuration file this session was opened with.
    #[cfg(unix)]
    pub(crate) fn config_path(&self) -> &Path {
        &self.build.launch.config_path
    }

    /// The context window the session's current model is configured with,
    /// when its catalog entry knows one.
    pub(crate) fn configured_context_window(&self) -> Option<u64> {
        let current = self.current_model();
        model_context_window(&current.provider, &current.model)
    }

    /// The session's workspace directory.
    pub(crate) fn cwd(&self) -> &Path {
        self.storage.cwd()
    }

    /// The session's project trust: what the environment it opened with
    /// granted.
    pub(crate) fn project_trust(&self) -> crate::workspace::ProjectTrusts {
        self.build.host.environment.project_trust()
    }

    /// The root the session's tools resolve paths against: the workspace
    /// as the permission layer canonicalized it.
    pub(crate) fn workspace_root(&self) -> &Path {
        &self.permission.workspace_root
    }

    pub(crate) fn permission_mode(&self) -> PermissionMode {
        self.permission.mode
    }

    /// The handle that cancels or steers this runtime's prompts and closes
    /// it. Hosts that publish a session share it.
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

    /// Which model prompt file the session runs on, in the words the
    /// in-process TUI reports after a model switch. `None` for a session
    /// whose prompt does not follow its model.
    #[cfg(unix)]
    pub(crate) fn model_prompt_notice(&self) -> Option<String> {
        self.built.prompt_template.as_ref().map(|_| {
            crate::app::model_prompt_metadata::provenance_notice(
                self.built.prompt.provenance.as_ref(),
            )
        })
    }

    /// Whether the session keeps a file at all, or lives in memory alone.
    pub(crate) fn persistence(&self) -> crate::session::SessionPersistence {
        self.storage.persistence()
    }

    /// Whether the session has saved its file yet: a new session saves it
    /// with its first prompt.
    pub(crate) fn is_saved(&self) -> bool {
        self.storage.file().is_some()
    }

    /// Tells the session's hooks what it does, for a host that reports
    /// what happens outside a prompt.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn hooks(&self) -> rho_sdk::hooks::HookDispatcher {
        self.built.runtime.hooks()
    }

    /// Hands the host the questions the session's subagents ask from now
    /// on, whether or not a prompt runs, as the in-process TUI's inbox
    /// takes them: a subagent outlives the prompt that started it, and may
    /// ask long after. `None`, and nothing changes, when nobody can be
    /// asked or the session starts no subagents. Call once.
    // Only the daemon, unix-only so far, takes them itself (#1665).
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn host_takes_child_questions(
        &mut self,
    ) -> Option<
        tokio::sync::mpsc::Receiver<crate::app::subagent_host_input::SubagentHostInputRequest>,
    > {
        match self.build.host.questions {
            QuestionSupport::Available => {}
            QuestionSupport::Unavailable => return None,
        }
        self.built.tools.subagents()?;
        let (sender, receiver) = tokio::sync::mpsc::channel(
            crate::app::subagent_host_input::SubagentHostInputBridge::QUEUE_CAPACITY,
        );
        self.child_questions = Some(sender);
        self.send_child_questions_to_host();
        Some(receiver)
    }

    /// Points the session's subagents, as its tools hold them now, at the
    /// host's question queue, when the host takes their questions.
    fn send_child_questions_to_host(&self) {
        if let (Some(sender), Some(manager)) = (&self.child_questions, self.built.tools.subagents())
        {
            manager.bind_host_input_to(sender.clone());
        }
    }

    /// Whether running background work holds a prompt open, or the host
    /// wakes the idle session once it finishes, as the session's host says
    /// (see [`BackgroundWait::of`]).
    pub(crate) fn background_wait(&self) -> BackgroundWait {
        BackgroundWait::of(self.build.host.children, self.build.host.idle_turns)
    }

    /// Whether background work the session's prompts started still runs or
    /// holds a result for the next prompt, which [`Self::shutdown`] would
    /// stop or drop (#1661).
    pub(crate) fn has_background_work(&self) -> bool {
        let session_id = self.built.session.id().to_string();
        delegation::BackgroundSources::of(&self.built.tools).has_work(&session_id)
            || self
                .held_notices
                .as_ref()
                .is_some_and(delegation::HeldNotices::has_undelivered)
    }

    /// Cancels any prompt and tears the session down.
    pub(crate) async fn shutdown(self) {
        self.handle.cancel();
        self.built.teardown().await;
    }

    /// Reports to the session's hooks that it ends with its prompt stopped
    /// for `reason`, as a failed run. A cancelled turn reports no session
    /// end, since an attended session goes on after it; a headless host,
    /// whose session ends with its one prompt, calls this before
    /// [`Self::shutdown`] when that prompt was cancelled.
    pub(crate) fn end_stopped(&self, reason: &str) {
        self.dispatch_failed(&format!("stopped: {reason}"));
    }

    fn dispatch_failed(&self, message: &str) {
        self.built.runtime.hooks().session_failed(
            self.built.session.id(),
            rho_sdk::hooks::HookSessionFailureKind::RunFailed,
            message,
        );
    }
}

/// The context window `estimate` fills: `configured`, the window the
/// session's model is configured with, else the one the provider reported
/// with `estimate`, as the TUI context meter measures it. `None` when
/// neither knows.
pub(crate) fn context_window(
    configured: Option<u64>,
    estimate: rho_sdk::ContextEstimate,
) -> Option<u64> {
    configured.or(estimate.reported_context_window())
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
