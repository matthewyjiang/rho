//! `rho acp`: an ACP agent over stdio that relays the rho daemon.
//!
//! The daemon runs every session; this process is one of its clients, with
//! one connection for all the sessions its host opens (`app::daemon_client`,
//! whose errors `daemon.rs` maps to ACP errors). Each ACP request becomes
//! daemon requests (`sessions.rs`, `prompt.rs`, `auth.rs`), and each
//! session's daemon events become ACP updates (`updates.rs`) and host
//! prompts (`inputs.rs`). A connection that negotiates ACP v2 is served by
//! translating at its boundary (`v2`).

mod auth;
mod daemon;
mod elicitation;
mod host_features;
mod inputs;
mod options;
mod permission;
mod prompt;
mod requests;
mod sessions;
mod updates;
mod v2;

use std::{collections::BTreeSet, future::Future, pin::Pin, sync::Arc};

use agent_client_protocol::{
    schema::ProtocolVersion,
    v1::schema::{
        AgentAuthCapabilities, AgentCapabilities, AuthenticateRequest, CancelNotification,
        CloseSessionRequest, CompleteElicitationNotification, CreateElicitationRequest,
        CreateElicitationResponse, DeleteSessionRequest, Implementation, InitializeRequest,
        InitializeResponse, ListSessionsRequest, ListSessionsResponse, LoadSessionRequest,
        LoadSessionResponse, LogoutCapabilities, LogoutRequest, McpCapabilities, NewSessionRequest,
        NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse,
        RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest,
        ResumeSessionResponse, SessionAdditionalDirectoriesCapabilities, SessionCapabilities,
        SessionCloseCapabilities, SessionDeleteCapabilities, SessionId, SessionListCapabilities,
        SessionNotification, SessionResumeCapabilities, SetSessionConfigOptionRequest,
        SetSessionModeRequest, StopReason,
    },
    v1::{ConnectionTo, Responder},
    Agent, Client, ConnectTo, Error as AcpError, JsonRpcResponse, Role, Stdio,
};
use rho_protocol::{methods::SessionList, ChildThreads, SessionListParams, SessionPart};

use super::{
    daemon_client::Daemon,
    shutdown_signal::{ShutdownSignals, TerminalClosed},
};
use crate::{
    cli::Cli,
    herdr::{HerdrReporter, HerdrSource},
};
use auth::LoginFlight;
use host_features::NegotiatedFeatures;
use requests::Launch;
use sessions::{Sessions, Started};

/// Outbound ACP host port: session updates, permission prompts, and
/// elicitations, as one connection's protocol version sends them.
///
/// Implemented by [`ConnectionPort`] for v1 and `v2::port::V2Port` for v2.
/// Keep it object-safe. Dropping a returned request future before it
/// resolves abandons the request; the port then sends `$/cancel_request`.
pub(super) trait AcpClientPort: Send + Sync {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>>;

    fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>;

    /// `elicitation/create`, sent only to hosts that advertised its mode.
    fn create_elicitation(
        &self,
        request: CreateElicitationRequest,
    ) -> Pin<Box<dyn Future<Output = Result<CreateElicitationResponse, AcpError>> + Send + '_>>;

    /// `elicitation/complete`: the out-of-band step behind a URL-mode
    /// elicitation finished.
    fn complete_elicitation(
        &self,
        notification: CompleteElicitationNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>>;

    /// The turn of session `session_id` that the host was told is running
    /// when it attached, which no prompt of its own runs, ended as `end`.
    /// Only ACP v2 reports turn state (`state_update`), so a v1 host has
    /// nothing to hear.
    fn attached_turn_ended(&self, _session_id: &SessionId, _end: TurnEnd) {}
}

/// How a turn ended, as far as the host can tell.
pub(super) enum TurnEnd {
    /// With this stop reason, or the error it failed with.
    Ended(Result<StopReason, AcpError>),
    /// While the host was not following it, so how is unknown.
    Unseen,
}

/// The [`AcpClientPort`] of an ACP v1 connection.
struct ConnectionPort {
    cx: ConnectionTo<Client>,
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

/// One `rho acp` process: its daemon connection and the sessions its host
/// has open.
pub(super) struct Adapter {
    daemon: Arc<Daemon>,
    launch: Launch,
    negotiated: NegotiatedFeatures,
    sessions: Sessions,
    herdr: HerdrReporter,
    logins: LoginFlight,
}

impl Adapter {
    fn new(cli: &Cli) -> anyhow::Result<Self> {
        let cwd = std::env::current_dir()?;
        let added_dirs = crate::added_dirs::launch_added_dirs(&cli.add_dirs, &cwd)?;
        let without = [
            (cli.no_tools, SessionPart::Tools),
            (cli.no_system_prompt, SessionPart::SystemPrompt),
            (cli.no_subagents, SessionPart::Subagents),
        ]
        .into_iter()
        .filter_map(|(left_out, part)| left_out.then_some(part))
        .collect::<BTreeSet<_>>();
        Ok(Self {
            // An editor's host answers its sessions' approvals.
            daemon: Arc::new(Daemon::for_frontend()),
            launch: Launch {
                agent: cli.agent.clone(),
                added_dirs: added_dirs.as_slice().to_vec(),
                without,
                child_threads: match &cli.command {
                    Some(crate::cli::Command::Acp(args)) if args.t3code => ChildThreads::Host,
                    _ => ChildThreads::Daemon,
                },
            },
            negotiated: NegotiatedFeatures::default(),
            sessions: Sessions::default(),
            // Headless, so Herdr does not treat the pane as an interactive
            // Rho.
            herdr: HerdrReporter::from_env().with_source(HerdrSource::Headless),
            logins: LoginFlight::default(),
        })
    }

    /// Keeps the host's features and connection from `initialize`.
    pub(super) fn record_client(&self, request: &InitializeRequest, port: Arc<dyn AcpClientPort>) {
        self.negotiated.record(request, port);
    }

    /// The v1 handshake. The v2 adapter translates the response.
    pub(super) async fn initialize(&self, request: &InitializeRequest) -> InitializeResponse {
        InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(
                AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(
                        PromptCapabilities::new()
                            .image(true)
                            .audio(false)
                            .embedded_context(true),
                    )
                    // Host stdio servers are the ACP baseline; Streamable
                    // HTTP is the transport Rho also speaks. SSE is not.
                    .mcp_capabilities(McpCapabilities::new().http(true))
                    // `additionalDirectories` on `session/new`, `session/load`,
                    // and `session/resume`; `session/close`; `session/list` and
                    // `session/delete` for host history.
                    .session_capabilities(
                        SessionCapabilities::new()
                            .additional_directories(SessionAdditionalDirectoriesCapabilities::new())
                            .resume(SessionResumeCapabilities::new())
                            .close(SessionCloseCapabilities::new())
                            .list(SessionListCapabilities::new())
                            .delete(SessionDeleteCapabilities::new()),
                    )
                    .auth(AgentAuthCapabilities::new().logout(LogoutCapabilities::new())),
            )
            .auth_methods(self.auth_methods(request).await)
            // Hosts display `version` as the agent version; it matches `rho --version`.
            .agent_info(Implementation::new("rho", env!("CARGO_PKG_VERSION")).title("Rho"))
    }

    /// `session/list`: the daemon's saved sessions, a page at a time.
    pub(super) async fn list_sessions(
        &self,
        request: &ListSessionsRequest,
    ) -> Result<ListSessionsResponse, AcpError> {
        let params = SessionListParams {
            cwd: request.cwd.clone(),
            cursor: request.cursor.clone(),
            limit: None,
        };
        let page = self.daemon.request::<SessionList>(&params).await?;
        Ok(requests::session_list(page))
    }

    fn toggles(&self) -> host_features::Toggles {
        self.negotiated.get().toggles
    }
}

/// The v1 responses to the requests that open a session, showing its
/// options as a host with `toggles` draws them.
impl Started {
    fn new_session_response(&self, toggles: host_features::Toggles) -> NewSessionResponse {
        NewSessionResponse::new(self.session_id.clone())
            .config_options(options::config_options(&self.options, toggles))
            .modes(options::modes(&self.options.permission_mode))
    }

    fn load_session_response(&self, toggles: host_features::Toggles) -> LoadSessionResponse {
        LoadSessionResponse::new()
            .config_options(options::config_options(&self.options, toggles))
            .modes(options::modes(&self.options.permission_mode))
    }

    fn resume_session_response(&self, toggles: host_features::Toggles) -> ResumeSessionResponse {
        ResumeSessionResponse::new()
            .config_options(options::config_options(&self.options, toggles))
            .modes(options::modes(&self.options.permission_mode))
    }
}

pub(super) async fn run(cli: Cli) -> anyhow::Result<()> {
    // Probe stdin before the transport starts reading it. See
    // `stdin_is_terminal`.
    let _ = crate::stdio::stdin_is_terminal();
    // Installed before the host can reach this agent, so a stop signal
    // always detaches its sessions.
    let mut stops = ShutdownSignals::install()?;
    let mut hangup = TerminalClosed::install()?;
    let adapter = Arc::new(Adapter::new(&cli)?);
    // Host integrations do not change protocol negotiation: each connection
    // uses the highest version both sides support.
    let server = Agent
        .protocol_router()
        .with_v1(v1_agent(Arc::clone(&adapter)))
        .with_v2(v2::agent(Arc::clone(&adapter)))
        .connect_to(Stdio::new());
    let served = tokio::select! {
        served = tokio::spawn(server) => served
            .map_err(|error| anyhow::anyhow!("ACP server task failed: {error}"))
            .and_then(|served| served.map_err(acp_error)),
        () = stopped(&mut stops, &mut hangup) => Ok(()),
    };
    // Cancels this host's prompts and detaches, so no turn of this host's
    // runs on unwatched; a second signal stops waiting for the daemon.
    tokio::select! {
        () = adapter.close_all() => {}
        () = stopped(&mut stops, &mut hangup) => {}
    }
    served
}

/// Returns once this process is told to stop: by a shutdown signal, or by
/// the terminal closing, as when a host closes the one it ran the agent in.
async fn stopped(stops: &mut ShutdownSignals, hangup: &mut TerminalClosed) {
    tokio::select! {
        _ = stops.recv() => {}
        () = hangup.recv() => {}
    }
}

/// The ACP v1 agent for one connection.
fn v1_agent(adapter: Arc<Adapter>) -> impl ConnectTo<Client> {
    Agent
        .v1()
        .name("rho")
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: InitializeRequest, responder, cx| {
                    adapter.record_client(&request, Arc::new(ConnectionPort { cx: cx.clone() }));
                    let adapter = Arc::clone(&adapter);
                    spawn_response(&cx, responder, async move {
                        Ok(adapter.initialize(&request).await)
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: NewSessionRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    let port: Arc<dyn AcpClientPort> = Arc::new(ConnectionPort { cx: cx.clone() });
                    cx.spawn(async move {
                        let started = adapter.new_session(&request).await;
                        let toggles = adapter.toggles();
                        respond_started(responder, &port, started, |started| {
                            started.new_session_response(toggles)
                        })
                        .await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: LoadSessionRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    let port: Arc<dyn AcpClientPort> = Arc::new(ConnectionPort { cx: cx.clone() });
                    cx.spawn(async move {
                        let started = adapter.load_session(&request, Arc::clone(&port)).await;
                        let toggles = adapter.toggles();
                        respond_started(responder, &port, started, |started| {
                            started.load_session_response(toggles)
                        })
                        .await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: ResumeSessionRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    let port: Arc<dyn AcpClientPort> = Arc::new(ConnectionPort { cx: cx.clone() });
                    cx.spawn(async move {
                        let started = adapter.resume_session(&request).await;
                        let toggles = adapter.toggles();
                        respond_started(responder, &port, started, |started| {
                            started.resume_session_response(toggles)
                        })
                        .await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: PromptRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    spawn_response(&cx, responder, async move {
                        let stop = adapter
                            .prompt(&request.session_id, &request.prompt, /*accept*/ None)
                            .await?;
                        Ok(PromptResponse::new(stop))
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: ListSessionsRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    spawn_response(&cx, responder, async move {
                        adapter.list_sessions(&request).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: DeleteSessionRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    spawn_response(&cx, responder, async move {
                        adapter.delete_session(&request).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: SetSessionConfigOptionRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    spawn_response(&cx, responder, async move {
                        adapter.set_config_option(&request).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: SetSessionModeRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    spawn_response(&cx, responder, async move {
                        adapter.set_session_mode(&request).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: CloseSessionRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    spawn_response(&cx, responder, async move {
                        adapter.close_session(&request).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |request: AuthenticateRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    let port = ConnectionPort { cx: cx.clone() };
                    let request_id = responder.id().clone();
                    // A host that cancels sign-in cancels the login with it.
                    let cancellation = responder.cancellation();
                    spawn_response(&cx, responder, async move {
                        let auth = &request.method_id.0;
                        adapter
                            .authenticate(auth, request_id, &port, cancellation)
                            .await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let adapter = Arc::clone(&adapter);
                async move |_request: LogoutRequest, responder, cx| {
                    let adapter = Arc::clone(&adapter);
                    spawn_response(&cx, responder, async move { adapter.logout().await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let adapter = Arc::clone(&adapter);
                async move |notification: CancelNotification, _cx| {
                    adapter.cancel(&notification.session_id).await;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_close({
            let adapter = Arc::clone(&adapter);
            async move |_cx| {
                adapter.close_all().await;
                Ok(())
            }
        })
}

/// Answers `session/new`, `session/load`, or `session/resume` with
/// `respond(started)`, then sends the session's commands and lets its live
/// updates through: updates name a session the host learns from the
/// response, so they follow it.
async fn respond_started<Resp: JsonRpcResponse>(
    responder: Responder<Resp>,
    port: &Arc<dyn AcpClientPort>,
    started: Result<Started, AcpError>,
    respond: impl FnOnce(&Started) -> Resp,
) -> Result<(), AcpError> {
    let started = match started {
        Ok(started) => started,
        Err(error) => return responder.respond_with_error(error),
    };
    responder.respond(respond(&started))?;
    let commands = updates::notification(
        &started.session_id,
        updates::commands_update(&started.options.commands),
        updates::Delivery::Live,
    );
    if let Err(error) = port.send_session_notification(commands).await {
        tracing::warn!(%error, "could not advertise ACP commands");
    }
    started.open();
    Ok(())
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

/// `resource_not_found` as ACP v1 writes it, for a session the host names
/// that this adapter does not hold. The adapter speaks v1 on every
/// connection, so v2 hosts get the same error.
fn resource_not_found(uri: Option<String>) -> AcpError {
    agent_client_protocol::v1::schema::Error::resource_not_found(uri).into()
}

fn acp_error(error: AcpError) -> anyhow::Error {
    anyhow::Error::msg(error.to_string())
}
