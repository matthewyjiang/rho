mod agent;
mod auth;
mod auth_login;
mod commands;
mod compaction_card;
mod config_options;
mod elicitation;
mod events;
mod fast_mode;
mod host_features;
mod host_mcp;
mod permission;
mod session_host;
mod session_list;
mod signals;
mod skills;
mod startup;
mod t3_wake;
mod thought_level;

use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use agent_client_protocol::schema::v1::{DeleteSessionRequest, ListSessionsRequest};
use agent_client_protocol::{
    schema::v1::{
        AuthenticateRequest, CancelNotification, CloseSessionRequest,
        CompleteElicitationNotification, CreateElicitationRequest, CreateElicitationResponse,
        InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
        RequestPermissionRequest, RequestPermissionResponse, SessionNotification,
        SetSessionConfigOptionRequest, SetSessionModeRequest,
    },
    Agent, Client, ConnectionTo, Error as AcpError, JsonRpcResponse, Responder, Role, Stdio,
};

use crate::{config::Config, diagnostics::RuntimeDiagnostics, herdr::HerdrReporter};

use super::agent_binding::BoundAgent;
use agent::RhoAcpAgent;
pub(super) use host_features::HostFeatures;

/// The ACP client Rho integrates with, chosen at launch rather than inferred
/// from the tools a host happens to offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AcpHost {
    /// Any ACP client. Subagents run in-process.
    Generic,
    /// T3 Code (`rho acp --t3code`). Subagents run as T3 child threads.
    T3Code,
}

impl AcpHost {
    pub(super) fn from_args(args: &crate::cli::AcpArgs) -> Self {
        if args.t3code {
            Self::T3Code
        } else {
            Self::Generic
        }
    }
}

/// Prepared process state for a `rho acp` stdio server.
pub(super) struct AcpStartup {
    pub(super) host: AcpHost,
    pub(super) config: Config,
    pub(super) config_path: PathBuf,
    pub(super) cwd: PathBuf,
    pub(super) no_system_prompt: bool,
    pub(super) no_tools: bool,
    pub(super) no_subagents: bool,
    pub(super) agent: BoundAgent,
    pub(super) diagnostics: RuntimeDiagnostics,
    pub(super) herdr: HerdrReporter,
    /// `--add-dir` directories granted to every ACP session.
    pub(super) added_dirs: crate::added_dirs::AddedDirs,
    /// Optional features the host advertised at `initialize`.
    pub(super) host_features: HostFeatures,
}

#[cfg(test)]
impl AcpStartup {
    /// Startup for session-host fixtures: `config` bound to a plain root agent
    /// with every tool, rooted at `cwd`.
    pub(super) fn for_tests(config: Config, cwd: &std::path::Path) -> Self {
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
            &config,
        )
        .expect("bind test agent");
        Self {
            host: AcpHost::Generic,
            diagnostics: RuntimeDiagnostics::new(&config),
            config,
            config_path: cwd.join("rho-test-config.toml"),
            cwd: cwd.to_path_buf(),
            no_system_prompt: false,
            no_tools: false,
            no_subagents: false,
            agent,
            herdr: HerdrReporter::default(),
            added_dirs: Default::default(),
            host_features: HostFeatures::default(),
        }
    }
}

/// Outbound ACP host port used by session hosts and tests.
///
/// Implementors deliver session updates, permission prompts, and questions to
/// the editor host. Keep the trait object-safe so tests can inject fakes.
/// Dropping a returned request future before it resolves abandons the
/// request; the connection port then sends `$/cancel_request`.
pub(super) trait AcpClientPort: Send + Sync {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>>;

    fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>;

    /// `elicitation/create`. Session hosts send it only to hosts that
    /// advertised the request's mode; fakes that never see one keep this
    /// default.
    fn create_elicitation(
        &self,
        _request: CreateElicitationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<CreateElicitationResponse, AcpError>> + Send + '_>>
    {
        Box::pin(async { Err(AcpError::method_not_found()) })
    }

    /// `elicitation/complete`: the out-of-band step behind a URL-mode
    /// elicitation finished. Fakes that send no URL elicitation keep this.
    fn complete_elicitation(
        &self,
        _notification: CompleteElicitationNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

/// ACP connection adapter. Session hosts talk to this instead of the crate
/// connection type so tests can inject a fake port.
struct ConnectionPort {
    cx: ConnectionTo<Client>,
}

impl ConnectionPort {
    fn new(cx: ConnectionTo<Client>) -> Self {
        Self { cx }
    }
}

impl AcpClientPort for ConnectionPort {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        Box::pin(async move { self.cx.send_notification(notification) })
    }

    fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        Box::pin(async move { self.cx.send_request(request).block_task().await })
    }

    fn create_elicitation(
        &self,
        request: CreateElicitationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<CreateElicitationResponse, AcpError>> + Send + '_>>
    {
        Box::pin(async move { self.cx.send_request(request).block_task().await })
    }

    fn complete_elicitation(
        &self,
        notification: CompleteElicitationNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        Box::pin(async move { self.cx.send_notification(notification) })
    }
}

pub(super) async fn run(cli: crate::cli::Cli) -> anyhow::Result<()> {
    let agent = Arc::new(startup::LazyAgent::new(cli)?);
    Agent
        .builder()
        .name("rho")
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: InitializeRequest, responder, _cx| {
                    agent.record_client(&request);
                    responder.respond(RhoAcpAgent::initialize(&request, agent.oauth_sign_in()))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: NewSessionRequest, responder, cx| {
                    let agent = Arc::clone(&agent);
                    spawn_session_start(&cx, responder, async move {
                        agent.ready().await?.new_session(request).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: LoadSessionRequest, responder, cx| {
                    let agent = Arc::clone(&agent);
                    let port = ConnectionPort::new(cx.clone());
                    spawn_session_start(&cx, responder, async move {
                        agent.ready().await?.load_session(request, &port).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: PromptRequest, responder, cx| {
                    let agent = Arc::clone(&agent);
                    let port: Arc<dyn AcpClientPort> = Arc::new(ConnectionPort::new(cx.clone()));
                    spawn_response(&cx, responder, async move {
                        agent.ready().await?.prompt(request, port).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ListSessionsRequest, responder, cx| {
                spawn_response(&cx, responder, session_list::list(request))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: DeleteSessionRequest, responder, cx| {
                    let agent = Arc::clone(&agent);
                    spawn_response(&cx, responder, async move {
                        agent.delete_session(request).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: SetSessionConfigOptionRequest, responder, cx| {
                    let agent = Arc::clone(&agent);
                    let port = ConnectionPort::new(cx.clone());
                    spawn_response(&cx, responder, async move {
                        agent.ready().await?.set_config_option(request, &port).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: SetSessionModeRequest, responder, cx| {
                    let agent = Arc::clone(&agent);
                    let port = ConnectionPort::new(cx.clone());
                    spawn_response(&cx, responder, async move {
                        agent.ready().await?.set_session_mode(request, &port).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: CloseSessionRequest, responder, cx| {
                    let agent = Arc::clone(&agent);
                    spawn_response(&cx, responder, async move {
                        agent.ready().await?.close_session(request).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: AuthenticateRequest, responder, cx| {
                    let agent = Arc::clone(&agent);
                    let port = ConnectionPort::new(cx.clone());
                    let request_id = responder.id().clone();
                    // A host that cancels sign-in drops the login with it.
                    let cancellation = responder.cancellation();
                    spawn_response(&cx, responder, async move {
                        cancellation
                            .run_until_cancelled(agent.authenticate(request, request_id, &port))
                            .await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let agent = Arc::clone(&agent);
                async move |notification: CancelNotification, _cx| {
                    // Cancellation targets an ongoing prompt, not a future
                    // turn after session setup. No prompt exists before ready.
                    if let Some(agent) = agent.started() {
                        agent.cancel(notification).await;
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_close({
            let agent = Arc::clone(&agent);
            async move |_cx| {
                agent.shutdown().await;
                Ok(())
            }
        })
        .connect_to(Stdio::new())
        .await
        .map_err(acp_error)
}

fn spawn_response<Resp, Counterpart, Fut>(
    cx: &ConnectionTo<Counterpart>,
    responder: Responder<Resp>,
    work: Fut,
) -> Result<(), AcpError>
where
    Resp: JsonRpcResponse + Send + 'static,
    Counterpart: Role,
    Fut: Future<Output = Result<Resp, AcpError>> + Send + 'static,
{
    cx.spawn(async move {
        match work.await {
            Ok(response) => responder.respond(response),
            Err(error) => responder.respond_with_error(error),
        }
    })
}

/// Like [`spawn_response`] for `session/new` and `session/load`, then sends
/// the session's command advertisement. Updates name a session the client
/// learns from the response, so they follow it on the wire.
fn spawn_session_start<Resp, Fut>(
    cx: &ConnectionTo<Client>,
    responder: Responder<Resp>,
    work: Fut,
) -> Result<(), AcpError>
where
    Resp: JsonRpcResponse + Send + 'static,
    Fut: Future<Output = Result<agent::SessionStart<Resp>, AcpError>> + Send + 'static,
{
    let port = ConnectionPort::new(cx.clone());
    cx.spawn(async move {
        let agent::SessionStart {
            response,
            commands,
            publication,
        } = match work.await {
            Ok(started) => started,
            Err(error) => return responder.respond_with_error(error),
        };
        responder.respond(response)?;
        if let Err(error) = port.send_session_notification(commands).await {
            tracing::warn!(%error, "could not advertise ACP commands");
        } else {
            publication.sent();
        }
        Ok(())
    })
}

fn acp_error(error: AcpError) -> anyhow::Error {
    anyhow::Error::msg(error.to_string())
}
