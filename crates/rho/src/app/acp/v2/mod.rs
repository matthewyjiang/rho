//! ACP v2, served by translating at the connection boundary.
//!
//! Rho's agent and session hosts speak v1. A connection that negotiates v2
//! gets this builder instead of the v1 one: each v2 request is rewritten into
//! the v1 request the agent already handles ([`requests`]), the agent's v1
//! updates are rewritten into v2 on the way out ([`port`], [`updates`]), and
//! the prompt lifecycle, which v2 redesigned, wraps the v1 prompt
//! ([`prompt`]). v1 clients keep the unchanged v1 builder.

mod git_patch;
mod port;
mod prompt;
mod requests;
mod updates;

use std::{future::Future, sync::Arc};

use agent_client_protocol::schema::{v1, v2};
use agent_client_protocol::{
    Agent, Client, ConnectTo, Error as AcpError, JsonRpcResponse, Responder, V2ConnectionTo,
};
use serde::{de::DeserializeOwned, Serialize};

use super::{agent::RhoAcpAgent, session_list, startup::LazyAgent};
use port::V2Port;
use requests::Reattach;

/// The ACP v2 agent for one connection.
pub(super) fn agent(agent: Arc<LazyAgent>) -> impl ConnectTo<Client> {
    Agent
        .v2()
        .name("rho")
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::InitializeRequest,
                            responder: Responder<v2::InitializeResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let response = requests::initialize_request(&request).and_then(|request| {
                        agent.record_client(&request, Arc::new(V2Port::new(cx)));
                        requests::initialize_response(RhoAcpAgent::initialize(
                            &request,
                            agent.oauth_sign_in(),
                        ))
                    });
                    respond(responder, response)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::NewSessionRequest,
                            responder: Responder<v2::NewSessionResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let agent = Arc::clone(&agent);
                    cx.spawn(async move {
                        let started = async {
                            let request = requests::new_session_request(&request)?;
                            let start = agent.ready().await?.new_session(request).await?;
                            let setup =
                                requests::setup(start.response.config_options, start.commands)?;
                            let response = v2::NewSessionResponse::new(bridge::<_, v2::SessionId>(
                                &start.response.session_id,
                            )?)
                            .config_options(setup.config_options)
                            .available_commands(setup.available_commands);
                            Ok((response, start.publication))
                        };
                        match started.await {
                            Ok((response, publication)) => {
                                responder.respond(response)?;
                                publication.sent();
                                Ok(())
                            }
                            Err(error) => responder.respond_with_error(error),
                        }
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::ResumeSessionRequest,
                            responder: Responder<v2::ResumeSessionResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let agent = Arc::clone(&agent);
                    let port = V2Port::new(cx.clone());
                    cx.spawn(async move { resume(&agent, request, responder, &port).await })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::PromptRequest,
                            responder: Responder<v2::PromptResponse>,
                            cx: V2ConnectionTo<Client>| {
                    prompt::spawn(Arc::clone(&agent), request, responder, cx)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: v2::ListSessionsRequest,
                        responder: Responder<v2::ListSessionsResponse>,
                        cx: V2ConnectionTo<Client>| {
                spawn_response(&cx, responder, async move {
                    let response = session_list::list(bridge(&request)?).await?;
                    bridge(&response)
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::DeleteSessionRequest,
                            responder: Responder<v2::DeleteSessionResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let agent = Arc::clone(&agent);
                    spawn_response(&cx, responder, async move {
                        let response = agent.delete_session(bridge(&request)?).await?;
                        bridge(&response)
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::SetSessionConfigOptionRequest,
                            responder: Responder<v2::SetSessionConfigOptionResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let agent = Arc::clone(&agent);
                    let port = V2Port::new(cx.clone());
                    spawn_response(&cx, responder, async move {
                        let request = requests::set_config_option_request(&request)?;
                        let response = agent
                            .ready()
                            .await?
                            .set_config_option(request, &port)
                            .await?;
                        Ok(v2::SetSessionConfigOptionResponse::new(
                            updates::config_options(response.config_options)?,
                        ))
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::CloseSessionRequest,
                            responder: Responder<v2::CloseSessionResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let agent = Arc::clone(&agent);
                    spawn_response(&cx, responder, async move {
                        let response = agent
                            .ready()
                            .await?
                            .close_session(bridge(&request)?)
                            .await?;
                        bridge(&response)
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::LoginAuthRequest,
                            responder: Responder<v2::LoginAuthResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let agent = Arc::clone(&agent);
                    let port = V2Port::new(cx.clone());
                    let request_id = responder.id().clone();
                    // A host that cancels sign-in drops the login with it.
                    let cancellation = responder.cancellation();
                    spawn_response(&cx, responder, async move {
                        let request = v1::AuthenticateRequest::new(request.method_id.0.clone())
                            .meta(request.meta);
                        cancellation
                            .run_until_cancelled(agent.authenticate(request, request_id, &port))
                            .await?;
                        Ok(v2::LoginAuthResponse::new())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |_request: v2::LogoutAuthRequest,
                            responder: Responder<v2::LogoutAuthResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let agent = Arc::clone(&agent);
                    spawn_response(&cx, responder, async move {
                        agent.logout().await?;
                        Ok(v2::LogoutAuthResponse::new())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let agent = Arc::clone(&agent);
                async move |notification: v2::CancelSessionNotification,
                            _cx: V2ConnectionTo<Client>| {
                    // Cancellation targets an ongoing prompt; none exists
                    // before the agent is ready.
                    if let Some(agent) = agent.started() {
                        agent
                            .cancel(v1::CancelNotification::new(bridge::<_, v1::SessionId>(
                                &notification.session_id,
                            )?))
                            .await;
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
}

/// `session/resume`, replaying history first when asked. v2 requires a
/// current-state snapshot before a successful response; reopening a session
/// retires any turn it had, so the reopened session is idle.
async fn resume(
    agent: &LazyAgent,
    request: v2::ResumeSessionRequest,
    responder: Responder<v2::ResumeSessionResponse>,
    port: &V2Port,
) -> Result<(), AcpError> {
    let session_id = request.session_id.clone();
    let started = async {
        let agent = agent.ready().await?;
        let (config_options, start_commands, publication) =
            match requests::resume_session_request(&request)? {
                Reattach::Replay(request) => {
                    let start = agent.load_session(request, port).await?;
                    (
                        start.response.config_options,
                        start.commands,
                        start.publication,
                    )
                }
                Reattach::Resume(request) => {
                    let start = agent.resume_session(request).await?;
                    (
                        start.response.config_options,
                        start.commands,
                        start.publication,
                    )
                }
            };
        let setup = requests::setup(config_options, start_commands)?;
        Ok::<_, AcpError>((setup, publication))
    };
    let (setup, publication) = match started.await {
        Ok(started) => started,
        Err(error) => return responder.respond_with_error(error),
    };
    port.send_update(
        &session_id,
        v2::SessionUpdate::StateUpdate(v2::StateUpdate::Idle(v2::IdleStateUpdate::new())),
    )?;
    responder.respond(
        v2::ResumeSessionResponse::new()
            .config_options(setup.config_options)
            .available_commands(setup.available_commands),
    )?;
    publication.sent();
    Ok(())
}

fn spawn_response<Resp, Fut>(
    cx: &V2ConnectionTo<Client>,
    responder: Responder<Resp>,
    work: Fut,
) -> Result<(), AcpError>
where
    Resp: JsonRpcResponse + Send + 'static,
    Fut: Future<Output = Result<Resp, AcpError>> + Send + 'static,
{
    cx.spawn(async move { respond(responder, work.await) })
}

fn respond<Resp: JsonRpcResponse>(
    responder: Responder<Resp>,
    result: Result<Resp, AcpError>,
) -> Result<(), AcpError> {
    match result {
        Ok(response) => responder.respond(response),
        Err(error) => responder.respond_with_error(error),
    }
}

/// Converts between the v1 and v2 forms of a type whose wire shape v2 kept
/// or only widened (new optional fields, open enums).
fn bridge<From: Serialize, To: DeserializeOwned>(value: &From) -> Result<To, AcpError> {
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|error| {
            AcpError::internal_error().data(format!(
                "could not convert {} to {}: {error}",
                std::any::type_name::<From>(),
                std::any::type_name::<To>()
            ))
        })
}
