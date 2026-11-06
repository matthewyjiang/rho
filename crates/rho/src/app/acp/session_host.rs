//! One ACP session: the protocol side of a [`SessionRuntime`].
//!
//! The runtime owns the session itself: its build, file, settings, and
//! turns. The host owns what only ACP has: the client and its features, the
//! advertised menus, permission card placeholders, the checklist the client
//! last saw, T3 Code's wake watchers, and Herdr state. A prompt resolves its
//! command here, then calls the runtime's core for it; a model turn runs
//! through the runtime with a view of this host (`session_host_turns.rs`)
//! whose forwarder shows the turn's events to the client and answers its
//! requests (`session_host_forward.rs`).

use std::sync::atomic::AtomicU64;

use agent_client_protocol::{
    schema::v1::{
        PlanEntry, PromptRequest, PromptResponse, SessionConfigOption, SessionId,
        SessionNotification, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
        StopReason,
    },
    Error as AcpError,
};
use rho_providers::{credentials::available_auth_modes, model::catalog};
use rho_sdk::UserInput;

use super::{
    commands::{self, AcpPrompt},
    config_options,
    events::EventMapper,
    fast_mode,
    host_features::{Forms, HostFeatures, Toggles},
    permission,
    prompt_acceptance::PromptAcceptance,
    signals::SessionSignals,
    t3_wake::T3Wake,
    thought_level, AcpClientPort, AcpHost, AcpStartup,
};
use crate::{
    app::{
        headless_delegation::BackgroundWait,
        session_commands::{CommandScope, PromptCommand},
        session_runtime::{
            model_scoped_config, ApprovalPolicy, ChildAgents, CurrentModel, InsertedMessage,
            PromptScope, QuestionSupport, SessionBuild, SessionHandle, SessionRole, SessionRuntime,
            SettingChangeError, TitleJob, Turn, TurnOutcome, WorkflowStart,
        },
        workflow_cli::discover::StartCommand,
    },
    config::Config,
    credential_store::AppCredentialStore,
    herdr::{HerdrReporter, HerdrSession, HerdrState},
    permission::PermissionMode,
    tools::mcp::McpAuthorizationMode,
};

#[path = "session_host_advertisements.rs"]
mod advertisements;
#[path = "session_host_agent.rs"]
mod agent;
#[path = "session_host_compact.rs"]
mod compact;
#[path = "session_host_convert.rs"]
mod convert;
#[path = "session_host_forward.rs"]
mod forward;
#[path = "session_host_open.rs"]
mod open;
#[path = "session_host_turns.rs"]
mod turns;
#[path = "session_host_wake.rs"]
mod wake;

use convert::user_input_from_prompt;
pub(super) use convert::RejectedDirectory;
pub(super) use open::{History, ReopenRequest, Reopened};
use turns::AcpTurns;

/// A title one host asked for, not yet generated.
pub(in crate::app::acp) struct PendingTitle {
    pub(in crate::app::acp) session_id: SessionId,
    /// The asking host's runtime handle, which its published slot shares,
    /// so the agent can tell that host from a successor with the same ID.
    pub(in crate::app::acp) asking: SessionHandle,
    pub(in crate::app::acp) job: TitleJob,
}

pub(super) struct SessionHost {
    acp_session_id: SessionId,
    /// What the session's host does for its tools, shared by every tool set
    /// the session builds.
    client_io: super::client_io::ClientIo,
    advertisements: advertisements::Advertisements,
    permission_placeholders: AtomicU64,
    herdr: HerdrReporter,
    /// The todo checklist the client last received as an ACP `plan`.
    sent_plan: Vec<PlanEntry>,
    /// T3 Code's wake watcher (`--t3code`): background work that outlives a
    /// prompt wakes the idle thread instead of holding the prompt open.
    t3_wake: Option<T3Wake>,
    /// Optional features the host advertised at `initialize`.
    host_features: HostFeatures,
    /// How the running prompt reports its inserted user message; set as
    /// each prompt starts, and back to [`PromptAcceptance::Implicit`] after.
    prompt_acceptance: PromptAcceptance,
    runtime: SessionRuntime,
}

impl SessionHost {
    /// Runs one `session/prompt`: a slash command or a model turn. A
    /// command that cannot run fails with
    /// [`crate::app::session_runtime::CommandRejected`]. `startup` supplies
    /// process defaults and the available root capabilities when a fresh
    /// session binds an agent definition.
    ///
    /// `acceptance` reports the user message once it is inserted: when a
    /// model turn has saved it, or as a command without a turn starts. A
    /// prompt that fails or is cancelled first never reports it.
    pub(super) async fn prompt(
        &mut self,
        request: PromptRequest,
        acceptance: PromptAcceptance,
        client: &dyn AcpClientPort,
        startup: &AcpStartup,
    ) -> anyhow::Result<PromptResponse> {
        self.prompt_acceptance = acceptance;
        let result = self.dispatch_prompt(request, client, startup).await;
        self.prompt_acceptance = PromptAcceptance::Implicit;
        result
    }

    async fn dispatch_prompt(
        &mut self,
        request: PromptRequest,
        client: &dyn AcpClientPort,
        startup: &AcpStartup,
    ) -> anyhow::Result<PromptResponse> {
        let input = user_input_from_prompt(&request.prompt)?;
        let prompt = self.runtime.begin_prompt(&startup.config).await?;
        self.refresh_advertisements(&startup.config, client).await;
        let watcher_note = self.wake_prompt_started(&input);
        let _ = self
            .herdr
            .report_state(
                HerdrState::Working,
                None,
                Some(&HerdrSession::without_resume(
                    self.acp_session_id.0.as_ref(),
                )),
            )
            .await;
        let home = crate::paths::home_dir();
        let scope = command_scope(&self.runtime, home.as_deref(), &startup.config);
        // A watcher a host restart cut off resumes its wait, whatever the prompt says.
        let action = if let Some(signal) = self.recorded_wake() {
            Some(Ok(AcpPrompt::AwaitWake(signal)))
        } else {
            // Expansion can wait on an MCP server, so a cancel drops it.
            tokio::select! {
                biased;
                () = prompt.cancellation().cancelled() => None,
                action = commands::resolve(input.clone(), &scope) => Some(action),
            }
        };
        let shown = match action {
            None => Shown::delivered(Ok(PromptResponse::new(StopReason::Cancelled))),
            Some(Ok(AcpPrompt::Shared(PromptCommand::Turn {
                input: model,
                calls,
            }))) => {
                let model = match watcher_note {
                    Some(note) => append_text(model, note),
                    None => model,
                };
                let turn = Turn {
                    display: input.clone(),
                    model,
                    calls,
                };
                self.run_turn(&prompt, turn, &startup.config, client).await
            }
            Some(Ok(AcpPrompt::Shared(PromptCommand::Compact { instructions }))) => {
                self.prompt_acceptance.accept(InsertedMessage::Unreplayed);
                Shown::delivered(self.compact(&prompt, instructions, client).await)
            }
            Some(Ok(AcpPrompt::Shared(PromptCommand::StartWorkflow(start)))) => Shown::delivered(
                self.start_workflow(&prompt, &input, start, startup, client)
                    .await,
            ),
            Some(Ok(AcpPrompt::Shared(PromptCommand::RunAsAgent(task)))) => {
                self.run_as_agent(&prompt, &input, task, startup, client)
                    .await
            }
            Some(Ok(AcpPrompt::AwaitWake(signal))) => {
                self.prompt_acceptance.accept(InsertedMessage::Unreplayed);
                Shown::delivered(self.await_wake(signal, &prompt, client).await)
            }
            Some(Err(rejected)) => Shown::delivered(Err(rejected.into())),
        };
        // How the prompt's work ended, though the client missed an update.
        let stop = shown
            .ended
            .as_ref()
            .ok()
            .map(|response| response.stop_reason);
        let result = shown.into_result();
        // Every card update of the prompt went out, so terminals of finished
        // calls can go.
        self.client_io.release_finished_terminals().await;
        self.refresh_advertisements(&startup.config, client).await;
        self.wake_prompt_ended(stop, client).await;
        let _ = self
            .herdr
            .report_state(
                HerdrState::Idle,
                None,
                Some(&HerdrSession::without_resume(
                    self.acp_session_id.0.as_ref(),
                )),
            )
            .await;
        result
    }

    pub(super) async fn set_config_option(
        &mut self,
        request: SetSessionConfigOptionRequest,
        process_config: &Config,
    ) -> Result<SetSessionConfigOptionResponse, AcpError> {
        match request.config_id.0.as_ref() {
            permission::MODE_CONFIG_ID => {
                let mode = permission::parse_mode_config_request(&request)?;
                self.set_permission_mode(mode, process_config).await?;
            }
            thought_level::THOUGHT_LEVEL_ID => {
                let requested = thought_level::parse_thought_level_request(&request)?;
                self.runtime
                    .set_reasoning_level(requested, process_config)
                    .map_err(|error| setting_error(&self.acp_session_id, error))?;
            }
            fast_mode::FAST_MODE_ID => {
                let enabled =
                    fast_mode::parse_fast_mode_request(&request, self.host_features.toggles)?;
                self.runtime
                    .set_fast_mode(enabled)
                    .map_err(|error| setting_error(&self.acp_session_id, error))?;
            }
            _ => {
                return Ok(SetSessionConfigOptionResponse::new(
                    self.set_model_option(&request, process_config).await?,
                ));
            }
        }
        Ok(SetSessionConfigOptionResponse::new(
            self.config_options(process_config),
        ))
    }

    /// Applies `mode` to later prompts (`session/set_mode`, or the `mode`
    /// config option). The caller holds the host slot lock, so no prompt is
    /// in flight. On error the session keeps its old mode.
    pub(super) async fn set_permission_mode(
        &mut self,
        mode: PermissionMode,
        process_config: &Config,
    ) -> Result<(), AcpError> {
        self.runtime
            .set_permission_mode(mode, process_config)
            .await
            .map_err(|error| setting_error(&self.acp_session_id, error))
    }

    pub(super) fn handle(&self) -> SessionHandle {
        self.runtime.handle()
    }

    /// The title the last prompt asked for, for the agent to generate and
    /// report once the prompt returns.
    pub(in crate::app::acp) fn take_title(&mut self) -> Option<PendingTitle> {
        let job = self.runtime.take_title()?;
        Some(PendingTitle {
            session_id: self.acp_session_id.clone(),
            asking: self.runtime.handle(),
            job,
        })
    }

    pub(super) fn config_options(&self, process_config: &Config) -> Vec<SessionConfigOption> {
        runtime_config_options(&self.runtime, process_config, self.host_features.toggles)
    }

    /// Resolves and applies `session/set_config_option` on this host. The caller
    /// holds the host slot lock so no prompt is in flight.
    pub(super) async fn set_model_option(
        &mut self,
        request: &SetSessionConfigOptionRequest,
        process_config: &Config,
    ) -> Result<Vec<SessionConfigOption>, AcpError> {
        let current = self.runtime.current_model();
        let available_auths = available_auth_modes(&AppCredentialStore);
        let selection = config_options::resolve_model_value(request, &current, &available_auths)?;
        self.runtime
            .switch_model(&selection, process_config)
            .await
            .map_err(|error| setting_error(&self.acp_session_id, error))?;
        Ok(self.config_options(process_config))
    }

    pub(super) async fn shutdown(self) {
        let Self {
            runtime,
            herdr,
            client_io,
            ..
        } = self;
        runtime.shutdown().await;
        client_io.release_all_terminals().await;
        let _ = herdr.release().await;
    }

    /// The host for `runtime`, talking to its client through `client_io`,
    /// with the wake watcher its [`session_build`] came with.
    fn from_runtime(
        client_io: super::client_io::ClientIo,
        startup: &AcpStartup,
        runtime: SessionRuntime,
        t3_wake: Option<T3Wake>,
    ) -> Self {
        Self {
            acp_session_id: client_io.session_id.clone(),
            client_io,
            advertisements: Default::default(),
            permission_placeholders: AtomicU64::new(0),
            herdr: startup.herdr.clone(),
            sent_plan: Vec::new(),
            t3_wake,
            host_features: startup.host_features,
            prompt_acceptance: PromptAcceptance::Implicit,
            runtime,
        }
    }

    /// A prompt's model turns (see [`SessionRuntime::run_prompt_turn`]):
    /// `turn`, then the implementation turn an approved plan asks for, each
    /// shown to `client` as it runs.
    async fn run_turn(
        &mut self,
        prompt: &PromptScope,
        turn: Turn,
        process_config: &Config,
        client: &dyn AcpClientPort,
    ) -> Shown<PromptResponse> {
        let mut turns = AcpTurns {
            session_id: &self.acp_session_id,
            client,
            features: self.host_features,
            placeholders: &self.permission_placeholders,
            acceptance: &mut self.prompt_acceptance,
            sent_plan: &mut self.sent_plan,
            advertisements: &mut self.advertisements,
            missed_update: None,
        };
        let ended = self
            .runtime
            .run_prompt_turn(&mut turns, prompt, turn, process_config)
            .await;
        Shown {
            ended: ended.map(prompt_response),
            missed_update: turns.missed_update,
        }
    }

    /// `/workflow:<name>`: starts the run in the background and answers with
    /// the notice the prompt was saved with.
    async fn start_workflow(
        &mut self,
        prompt: &PromptScope,
        display: &UserInput,
        start: StartCommand,
        startup: &AcpStartup,
        client: &dyn AcpClientPort,
    ) -> anyhow::Result<PromptResponse> {
        let started = self
            .runtime
            .start_workflow(prompt, display, start, &startup.config)
            .await?;
        match started {
            WorkflowStart::Cancelled => Ok(PromptResponse::new(StopReason::Cancelled)),
            WorkflowStart::Started { notice, inserted } => {
                self.prompt_acceptance.accept(inserted);
                send_notification(
                    client,
                    EventMapper::agent_notice(&self.acp_session_id, &notice),
                )
                .await?;
                Ok(PromptResponse::new(StopReason::EndTurn))
            }
        }
    }
}

/// Signal tracking for the next update, starting from the checklist the
/// client last saw, which this takes from `sent_plan`. Hand the checklist
/// back once sent; one that never comes back only costs a resend of the
/// current list.
fn session_signals(runtime: &SessionRuntime, sent_plan: &mut Vec<PlanEntry>) -> SessionSignals {
    SessionSignals::new(
        runtime.context_gauge(),
        runtime.tools().todo_state(),
        std::mem::take(sent_plan),
    )
}

/// What commands can see of `runtime`'s session.
fn command_scope<'a>(
    runtime: &'a SessionRuntime,
    home: Option<&'a std::path::Path>,
    process_config: &'a Config,
) -> CommandScope<'a> {
    CommandScope {
        cwd: runtime.cwd(),
        home,
        tools: runtime.tools(),
        config: process_config,
        permission_mode: runtime.permission_mode(),
    }
}

/// Every option `runtime`'s session advertises now; see
/// [`advertised_config_options`].
fn runtime_config_options(
    runtime: &SessionRuntime,
    process_config: &Config,
    toggles: Toggles,
) -> Vec<SessionConfigOption> {
    advertised_config_options(
        &runtime.current_model(),
        process_config,
        CurrentSettings {
            permission_mode: runtime.permission_mode(),
            reasoning: runtime.reasoning_level(),
            fast: runtime.fast_mode(),
        },
        toggles,
    )
}

/// How every build of a session this host opens is set up, and the T3 wake
/// watcher that wakes it.
struct HostBuild {
    build: SessionBuild,
    /// Present exactly when the build waits for the host to wake it.
    t3_wake: Option<T3Wake>,
}

/// How every build of a session this host opens is set up: the launch
/// flags, what this host does for its tools, and whether it wakes the
/// session itself.
fn session_build(startup: &AcpStartup, io: &super::client_io::ClientIo) -> HostBuild {
    // `--t3code`: a wake watcher wakes the idle thread when background work
    // that outlived its prompt finishes. Without a Rho home for its signals
    // nothing would wake the session, so background work holds the prompt
    // open instead.
    let t3_wake = match startup.host {
        AcpHost::Generic => None,
        AcpHost::T3Code => match super::t3_wake::signal_dir() {
            Ok(dir) => Some(T3Wake::new(dir)),
            Err(error) => {
                tracing::warn!(%error, "T3 wake watchers are off: no Rho home");
                None
            }
        },
    };
    let background_wait = match t3_wake {
        Some(_) => BackgroundWait::HostWakes,
        None => BackgroundWait::HoldPrompt,
    };
    let build = SessionBuild {
        config_path: startup.config_path.clone(),
        no_system_prompt: startup.no_system_prompt,
        no_tools: startup.no_tools,
        no_subagents: startup.no_subagents,
        diagnostics: startup.diagnostics.clone(),
        // Questions reach a person only through form elicitation.
        questions: question_support(startup.host_features.forms),
        // `--t3code`: the `agent` tool delegates to T3 so its UI owns children.
        children: match startup.host {
            AcpHost::Generic => ChildAgents::InProcess,
            AcpHost::T3Code => ChildAgents::HostThreads,
        },
        background_wait,
        launch_agent: startup.agent.clone(),
        host_io: io.host_io(),
        max_steps: None,
        role: SessionRole::Acp,
        hook_labels: Default::default(),
        prompt_suffix: None,
        // An ACP agent has no terminal for a person to finish an MCP login
        // in: its stdin is the protocol pipe.
        mcp_authorization: McpAuthorizationMode::NonInteractive,
        // A person answers through the client's permission prompts.
        approvals: ApprovalPolicy::Host,
    };
    HostBuild { build, t3_wake }
}

fn question_support(forms: Forms) -> QuestionSupport {
    match forms {
        Forms::Available(_) => QuestionSupport::Available,
        Forms::Unavailable => QuestionSupport::Unavailable,
    }
}

/// Session settings the advertised options report as current values.
#[derive(Clone, Copy)]
struct CurrentSettings {
    permission_mode: PermissionMode,
    reasoning: rho_sdk::ReasoningLevel,
    fast: bool,
}

/// Every option a session advertises, rebuilt after each change so a model
/// switch adds or drops the per-model `thought_level` and `fast_mode`. The
/// permission `mode` comes first, as the ACP spec's own example orders it.
fn advertised_config_options(
    current: &CurrentModel,
    process_config: &Config,
    settings: CurrentSettings,
    toggles: Toggles,
) -> Vec<SessionConfigOption> {
    let available_auths = available_auth_modes(&AppCredentialStore);
    let mut options = vec![permission::mode_config_option(settings.permission_mode)];
    options.extend(config_options::model_config_options(
        current,
        &process_config.favorite_models,
        catalog::available_models_for_auths(&available_auths),
    ));
    options.extend(thought_level::config_options(
        &model_scoped_config(current, process_config),
        settings.reasoning,
    ));
    options.extend(fast_mode::config_options(current, settings.fast, toggles));
    options
}

/// How a turn, or the prompt it answers, ended, and the first update of it
/// the client missed. The turn stands without that update: it was saved, and
/// the runtime judges it (plan handoff, title) by how it ended. The miss
/// fails the prompt only as the prompt answers.
struct Shown<T> {
    ended: anyhow::Result<T>,
    missed_update: Option<String>,
}

impl<T> Shown<T> {
    /// An end the client missed nothing of.
    fn delivered(ended: anyhow::Result<T>) -> Self {
        Self {
            ended,
            missed_update: None,
        }
    }

    /// The prompt's answer: the turn's own failure first, then a miss.
    fn into_result(self) -> anyhow::Result<T> {
        let ended = self.ended?;
        match self.missed_update {
            Some(message) => Err(anyhow::Error::msg(message)),
            None => Ok(ended),
        }
    }
}

/// The answer to a prompt whose last turn ended as `outcome`.
fn prompt_response(outcome: TurnOutcome) -> PromptResponse {
    let stop = match outcome {
        TurnOutcome::Completed(outcome) => EventMapper::map_stop(&outcome),
        TurnOutcome::Cancelled => StopReason::Cancelled,
    };
    PromptResponse::new(stop)
}

/// The ACP error for every refused setting change: permission mode, model,
/// thought level, or fast mode. An unsupported setting is the client's to
/// fix, a busy session's to send again once its turn ends, and a failed
/// apply is the agent's.
fn setting_error(session_id: &SessionId, error: SettingChangeError) -> AcpError {
    match error {
        SettingChangeError::Unsupported(_) => AcpError::invalid_params().data(error.to_string()),
        SettingChangeError::Busy => super::agent::busy_session(session_id),
        SettingChangeError::Apply(_) => AcpError::internal_error().data(error.to_string()),
    }
}

/// `input` with `text` as a final block.
fn append_text(input: UserInput, text: &str) -> UserInput {
    let mut blocks = input.blocks().to_vec();
    blocks.push(rho_sdk::model::ContentBlock::Text(text.to_owned()));
    UserInput::content(blocks).expect("adding a text block keeps the input valid")
}

async fn send_notification(
    client: &dyn AcpClientPort,
    notification: SessionNotification,
) -> anyhow::Result<()> {
    client
        .send_session_notification(notification)
        .await
        .map_err(|error| anyhow::Error::msg(error.to_string()))
}

#[cfg(test)]
impl SessionHost {
    /// A tool-less host for session `id` on `provider`, unsaved like a
    /// `session/new` host until its first save creates its file under `root`,
    /// which is also its workspace. Agent tests use it where a vacant slot
    /// cannot run a prompt.
    pub(super) async fn unsaved_for_tests(
        startup: &AcpStartup,
        root: &std::path::Path,
        id: rho_sdk::SessionId,
        provider: std::sync::Arc<dyn rho_sdk::provider::ModelProvider>,
    ) -> Self {
        use std::sync::Arc;

        use crate::app::{
            session_assembly::BuiltSession,
            session_runtime::{SessionPermission, SessionStorage},
        };
        let runtime = rho_sdk::Rho::builder()
            .provider_shared(Arc::clone(&provider))
            .build()
            .expect("test runtime");
        let session = runtime
            .session(rho_sdk::SessionOptions::new().id(id))
            .await
            .expect("test session");
        let id = session.id().to_string();
        let (store_root, store_id) = (root.to_path_buf(), id.clone());
        let storage = SessionStorage::unsaved(
            root.to_path_buf(),
            Box::new(move || {
                crate::session::Session::create_with_id_in_root_for_test(
                    &store_root,
                    &store_root,
                    &store_id,
                )
            }),
        );
        let built = BuiltSession {
            runtime,
            session,
            provider,
            tools: crate::tools::sdk_registry::AppToolSet::disabled(),
            hooks: None,
            approval_receiver: None,
            prompt_template: None,
            prompt: Default::default(),
            diagnostics: crate::diagnostics::RuntimeDiagnostics::new(&startup.config),
        };
        let permission = SessionPermission {
            mode: startup.config.permission_mode,
            writes: Default::default(),
            workspace_root: root.to_path_buf(),
        };
        let io = super::client_io::ClientIo::local(SessionId::new(id));
        Self::for_tests(io, startup, built, storage, permission)
    }

    /// A host for `built`, saved through `storage`, opened by `startup` with
    /// `permission`, for fixtures that script their own session. It runs as
    /// the launch agent, with none of the client's MCP servers or roots.
    pub(super) fn for_tests(
        io: super::client_io::ClientIo,
        startup: &AcpStartup,
        built: crate::app::session_assembly::BuiltSession,
        storage: crate::app::session_runtime::SessionStorage,
        permission: crate::app::session_runtime::SessionPermission,
    ) -> Self {
        let HostBuild { build, t3_wake } = session_build(startup, &io);
        let runtime = SessionRuntime::from_built_for_tests(
            build,
            built,
            storage,
            permission,
            startup.config.auth.clone(),
            &startup.logouts,
        );
        Self::from_runtime(io, startup, runtime, t3_wake)
    }
}

#[cfg(test)]
#[path = "session_host_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "session_host_delegation_tests.rs"]
mod delegation_tests;

#[cfg(test)]
#[path = "session_host_mode_tests.rs"]
mod mode_tests;

#[cfg(test)]
#[path = "session_host_tool_lifecycle_tests.rs"]
mod tool_lifecycle_tests;

#[cfg(test)]
#[path = "session_host_questions_tests.rs"]
mod questions_tests;

#[cfg(test)]
#[path = "session_host_persist_tests.rs"]
mod persist_tests;
