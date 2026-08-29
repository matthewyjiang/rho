//! User-initiated aside session: frozen parent snapshot plus read-only tools.

use std::{collections::BTreeSet, num::NonZeroUsize, path::PathBuf, sync::Arc};

use rho_sdk::{
    model::{ContentBlock, Message},
    SessionId, SessionOptions, UserInput,
};
use tokio::sync::mpsc;

use super::{
    agent_binding::{AgentBinder, AgentInvocation, AgentRole, BoundAgent},
    session_assembly::{
        assemble_session, ApprovalInputs, BuiltSession, SessionApproval, SessionAssemblyOptions,
    },
    tools_prompt::{McpAttach, McpSamplingSupport},
};
use crate::{
    agent::{
        AgentCapabilities, AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, PromptPolicy,
        ToolCapability, ToolPolicy,
    },
    config::Config,
    diagnostics::RuntimeDiagnostics,
    permission::PermissionMode,
    tools::agent::BackgroundSubagents,
};

pub(crate) const USAGE_PURPOSE: &str = "side";

pub(crate) const SIDE_CHAT_PROMPT: &str = "\
You are answering a user aside in a Rho coding session. A frozen snapshot of \
the parent conversation is already in this session's history. It is background, \
not a question.

You may use read-only tools to inspect the workspace: list_dir, read_file, \
grep, and glob. Do not edit files, run shell commands, or change the parent \
session.

Answer the user's aside directly and concisely.";

const SIDE_CHAT_STEP_LIMIT: usize = 16;

#[derive(Debug)]
pub(crate) enum SideChatCommand {
    Submit(String),
    Cancel,
    Shutdown,
}

#[derive(Debug)]
pub(crate) enum SideChatEvent {
    AssistantDelta(String),
    AssistantReset,
    ToolStarted(String),
    Finished,
    Failed(String),
    /// A submit arrived while a turn is still running. Overlay stays busy.
    Rejected(String),
    Cancelled,
}

pub(crate) struct SideChatLaunch {
    pub config: Config,
    pub config_path: PathBuf,
    pub cwd: PathBuf,
    pub parent_session_id: SessionId,
    pub snapshot: String,
}

pub(crate) struct SideChatHandle {
    commands: mpsc::UnboundedSender<SideChatCommand>,
    events: mpsc::UnboundedReceiver<SideChatEvent>,
}

impl SideChatHandle {
    pub(crate) fn submit(&self, prompt: String) {
        let _ = self.commands.send(SideChatCommand::Submit(prompt));
    }

    pub(crate) fn cancel(&self) {
        let _ = self.commands.send(SideChatCommand::Cancel);
    }

    pub(crate) fn try_recv(&mut self) -> Option<SideChatEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for SideChatHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(SideChatCommand::Shutdown);
    }
}

pub(crate) fn spawn_side_chat(launch: SideChatLaunch) -> SideChatHandle {
    let (commands, command_rx) = mpsc::unbounded_channel();
    let (event_tx, events) = mpsc::unbounded_channel();
    tokio::spawn(side_chat_worker(launch, command_rx, event_tx));
    SideChatHandle { commands, events }
}

async fn side_chat_worker(
    launch: SideChatLaunch,
    mut commands: mpsc::UnboundedReceiver<SideChatCommand>,
    events: mpsc::UnboundedSender<SideChatEvent>,
) {
    let mut built: Option<BuiltSession> = None;
    let mut active: Option<rho_sdk::Run> = None;
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    SideChatCommand::Shutdown => break,
                    SideChatCommand::Cancel => {
                        if let Some(run) = &active {
                            run.cancel();
                        }
                    }
                    SideChatCommand::Submit(prompt) => {
                        if active.is_some() {
                            let _ = events.send(SideChatEvent::Rejected(
                                "could not start side chat: a turn is already running".into(),
                            ));
                            continue;
                        }
                        if built.is_none() {
                            match assemble_side_session(&launch).await {
                                Ok(session) => built = Some(session),
                                Err(error) => {
                                    let _ = events.send(SideChatEvent::Failed(format!(
                                        "could not start side chat: {error}"
                                    )));
                                    continue;
                                }
                            }
                        }
                        let Some(session) = built.as_ref() else {
                            continue;
                        };
                        match session.session.start(UserInput::text(prompt)).await {
                            Ok(run) => active = Some(run),
                            Err(error) => {
                                let _ = events.send(SideChatEvent::Failed(format!(
                                    "could not start side chat: {error}"
                                )));
                            }
                        }
                    }
                }
            }
            event = next_run_event(&mut active) => {
                match event {
                    Some(event) => {
                        if apply_run_event(event, &events) {
                            active = None;
                        }
                    }
                    None => active = None,
                }
            }
        }
    }
    if let Some(session) = built.take() {
        session.teardown().await;
    }
}

async fn next_run_event(active: &mut Option<rho_sdk::Run>) -> Option<rho_sdk::RunEvent> {
    match active.as_mut() {
        Some(run) => run.next_event().await,
        None => std::future::pending().await,
    }
}

/// Forwards a run event. Returns true when the run is terminal.
fn apply_run_event(
    event: rho_sdk::RunEvent,
    events: &mpsc::UnboundedSender<SideChatEvent>,
) -> bool {
    match event {
        rho_sdk::RunEvent::AssistantTextDelta { text } => {
            let _ = events.send(SideChatEvent::AssistantDelta(text));
            false
        }
        rho_sdk::RunEvent::ProviderStreamReset { .. } => {
            let _ = events.send(SideChatEvent::AssistantReset);
            false
        }
        rho_sdk::RunEvent::ToolStarted { name, .. } => {
            let _ = events.send(SideChatEvent::ToolStarted(name));
            false
        }
        rho_sdk::RunEvent::Completed { .. } => {
            let _ = events.send(SideChatEvent::Finished);
            true
        }
        rho_sdk::RunEvent::Cancelled { .. } => {
            let _ = events.send(SideChatEvent::Cancelled);
            true
        }
        rho_sdk::RunEvent::Failed { message, .. } => {
            let _ = events.send(SideChatEvent::Failed(format!(
                "could not complete side chat: {message}"
            )));
            true
        }
        rho_sdk::RunEvent::HostInputRequested { .. }
        | rho_sdk::RunEvent::ToolHostInputRequested { .. } => {
            let _ = events.send(SideChatEvent::Failed(
                "could not complete side chat: host input is not available in side chat".into(),
            ));
            true
        }
        _ => false,
    }
}

async fn assemble_side_session(launch: &SideChatLaunch) -> anyhow::Result<BuiltSession> {
    let mut config = launch.config.clone();
    config.mcp = crate::tools::mcp::config::McpConfig::default();
    config.permission_mode = PermissionMode::Plan;
    let diagnostics = RuntimeDiagnostics::new(&config);
    let agent = bind_side_agent(&config)?;
    let snapshot = launch.snapshot.clone();
    let assembled = assemble_session(SessionAssemblyOptions {
        config: &config,
        config_path: launch.config_path.clone(),
        cwd: &launch.cwd,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: true,
        questionnaire_enabled: false,
        mcp_elicitation: crate::tools::mcp::McpElicitationSupport::Unavailable,
        mcp_sampling: McpSamplingSupport::Unavailable,
        mcp_attach: McpAttach::None,
        background_subagents: BackgroundSubagents::Disabled,
        diagnostics: &diagnostics,
        agent: &agent,
        max_steps: NonZeroUsize::new(SIDE_CHAT_STEP_LIMIT),
        usage_purpose: USAGE_PURPOSE,
        usage_parent_session_id: Some(launch.parent_session_id.clone()),
        hook_host_labels: rho_sdk::hooks::HookHostLabels::new(),
        extend_tools: |tools| tools,
        approval: |_: ApprovalInputs| {
            Ok(SessionApproval {
                session: None,
                receiver: None,
            })
        },
        session_options: move |_| {
            Ok(SessionOptions::new()
                .id(SessionId::new())
                .history(vec![Message::User(vec![ContentBlock::Text(snapshot)])]))
        },
    })
    .await?;
    Ok(assembled.built)
}

fn bind_side_agent(host_config: &Config) -> anyhow::Result<BoundAgent> {
    let tools = BTreeSet::from([
        ToolCapability::ListDir,
        ToolCapability::ReadFile,
        ToolCapability::Grep,
        ToolCapability::Glob,
    ]);
    let definition = Arc::new(AgentDefinition {
        id: AgentId::new("side").expect("valid side-chat agent id"),
        description: "User-initiated read-only aside from /side.".into(),
        prompt: PromptPolicy::Replace(SIDE_CHAT_PROMPT.into()),
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(tools.clone()),
            model: ModelPolicy::Inherit,
            reasoning: None,
        },
    });
    AgentBinder::bind(
        definition,
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: AgentCapabilities::new(tools),
        },
        host_config,
    )
}
