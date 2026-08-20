mod agent;
mod config_options;
mod events;
mod permission;
mod session_host;

use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use agent_client_protocol::{
    schema::v1::{
        AuthenticateRequest, CancelNotification, InitializeRequest, LoadSessionRequest,
        NewSessionRequest, PromptRequest, RequestPermissionRequest, RequestPermissionResponse,
        SessionNotification, SetSessionConfigOptionRequest, SetSessionModeRequest,
    },
    Agent, Client, ConnectionTo, Error as AcpError, JsonRpcResponse, Responder, Role, Stdio,
};

use crate::{config::Config, diagnostics::RuntimeDiagnostics, herdr::HerdrReporter};

use super::agent_binding::BoundAgent;
use agent::RhoAcpAgent;

/// Prepared process state for a `rho acp` stdio server.
pub(super) struct AcpStartup {
    pub(super) config: Config,
    pub(super) config_path: PathBuf,
    pub(super) cwd: PathBuf,
    pub(super) no_system_prompt: bool,
    pub(super) no_tools: bool,
    pub(super) no_subagents: bool,
    pub(super) agent: BoundAgent,
    pub(super) diagnostics: RuntimeDiagnostics,
    pub(super) herdr: HerdrReporter,
}

/// Outbound ACP host port used by session hosts and tests.
///
/// Implementors deliver session updates and permission prompts to the editor
/// host. Keep the trait object-safe so tests can inject fakes.
pub(super) trait AcpClientPort: Send + Sync {
    fn send_session_notification(
        &self,
        notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>>;

    fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>;
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
}

pub(super) async fn run(startup: AcpStartup) -> anyhow::Result<()> {
    let agent = Arc::new(RhoAcpAgent::new(startup));
    Agent
        .builder()
        .name("rho")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _cx| {
                responder.respond(RhoAcpAgent::initialize(&request))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: NewSessionRequest, responder, cx| {
                    let agent = Arc::clone(&agent);
                    spawn_response(
                        &cx,
                        responder,
                        async move { agent.new_session(request).await },
                    )
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
                    spawn_response(&cx, responder, async move {
                        agent.load_session(request, &port).await
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
                    let port = ConnectionPort::new(cx.clone());
                    spawn_response(
                        &cx,
                        responder,
                        async move { agent.prompt(request, &port).await },
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: SetSessionConfigOptionRequest, responder, cx| {
                    let agent = Arc::clone(&agent);
                    spawn_response(&cx, responder, async move {
                        agent.set_config_option(request).await
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionModeRequest, responder, _cx| {
                responder.respond_with_error(RhoAcpAgent::set_session_mode(&request))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: AuthenticateRequest, responder, _cx| {
                responder.respond_with_error(RhoAcpAgent::authenticate(&request))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let agent = Arc::clone(&agent);
                async move |notification: CancelNotification, _cx| {
                    agent.cancel(notification).await;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_close({
            let agent = Arc::clone(&agent);
            async move |_cx| {
                agent.shutdown_all().await;
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

fn acp_error(error: AcpError) -> anyhow::Error {
    anyhow::Error::msg(error.to_string())
}
