//! ACP v2, served by translating at the connection boundary.
//!
//! Rho's adapter speaks v1. A connection that negotiates v2 gets this
//! builder instead of the v1 one: each v2 request is rewritten into the v1
//! request the adapter already handles ([`requests`]), the adapter's v1
//! updates are rewritten into v2 on the way out ([`port`], [`updates`]), and
//! the prompt lifecycle, which v2 redesigned, wraps the v1 prompt
//! ([`prompt`]). v1 clients keep the unchanged v1 builder. Every
//! `state_update` lists the session's background work, which the host may
//! cancel piece by piece ([`background`]).

mod background;
mod errors;
mod git_patch;
mod port;
mod prompt;
mod requests;
mod updates;

use std::{future::Future, sync::Arc};

use agent_client_protocol::{v1::schema as v1, v2::schema as v2};
use agent_client_protocol::{
    v2::{ConnectionTo as V2ConnectionTo, Responder},
    Agent, Client, ConnectTo, Error as AcpError, JsonRpcResponse,
};
use serde::{de::DeserializeOwned, Serialize};

use super::{
    host_features::Turns,
    sessions::{IfOpen, Started},
    Adapter, Foreground,
};
use port::V2Port;
use requests::Reattach;

/// The ACP v2 agent for one connection.
pub(super) fn agent(agent: Arc<Adapter>) -> impl ConnectTo<Client> {
    Agent
        .v2()
        .name("rho")
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::InitializeRequest,
                            responder: Responder<v2::InitializeResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let request = match requests::initialize_request(&request) {
                        Ok(request) => request,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    let port = Arc::new(V2Port::new(cx.clone()));
                    let agent = Arc::clone(&agent);
                    spawn_response(&cx, responder, async move {
                        let response =
                            requests::initialize_response(agent.initialize(&request).await)?;
                        // The host may repeat `initialize` before its first
                        // other request; the latest that succeeded counts.
                        agent.negotiated.replace(&request, port, Turns::Any);
                        Ok(response)
                    })
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
                            let started = agent.new_session(&request).await?;
                            let setup = requests::setup(&started.options, agent.toggles())?;
                            let response = v2::NewSessionResponse::new(bridge::<_, v2::SessionId>(
                                &started.session_id,
                            )?)
                            .config_options(setup.config_options)
                            .available_commands(setup.available_commands);
                            Ok::<_, AcpError>((response, started))
                        };
                        match started.await {
                            Ok((response, started)) => {
                                responder.respond(response)?;
                                started.open();
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
                    let port = Arc::new(V2Port::replaying(cx.clone()));
                    let live = V2Port::new(cx.clone());
                    let responder = errors::session_scoped(responder, &request.session_id);
                    cx.spawn(async move { resume(&agent, request, responder, port, &live).await })
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
                    let responder = errors::session_scoped(responder, &request.session_id);
                    prompt::spawn(Arc::clone(&agent), request, responder, cx)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::ListSessionsRequest,
                            responder: Responder<v2::ListSessionsResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let agent = Arc::clone(&agent);
                    spawn_response(&cx, responder, async move {
                        let response = agent.list_sessions(&bridge(&request)?).await?;
                        bridge(&response)
                    })
                }
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
                        let response = agent.delete_session(&bridge(&request)?).await?;
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
                    let responder = errors::session_scoped(responder, &request.session_id);
                    spawn_response(&cx, responder, async move {
                        let request = requests::set_config_option_request(&request)?;
                        let response = agent.set_config_option(&request).await?;
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
                        let response = agent.close_session(&bridge(&request)?).await?;
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
                    // The v1 adapter scopes its elicitations by the v1 form of
                    // the same wire id.
                    let request_id = v1::RequestId::from(responder.raw_id().clone());
                    // A host that cancels sign-in cancels the login with it.
                    let cancellation = responder.cancellation();
                    spawn_response(&cx, responder, async move {
                        let auth = &request.method_id.0;
                        agent
                            .authenticate(auth, request_id, &port, cancellation)
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
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::CancelTerminalRequest,
                            responder: Responder<v2::CancelTerminalResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let agent = Arc::clone(&agent);
                    let responder = errors::session_scoped(responder, &request.session_id);
                    spawn_response(&cx, responder, async move {
                        let session_id = bridge::<_, v1::SessionId>(&request.session_id)?;
                        let terminal = request.terminal_id.0.to_string();
                        agent
                            .stop_background(&session_id, |work| {
                                background::terminal(work, &terminal)
                            })
                            .await?;
                        Ok(v2::CancelTerminalResponse::new())
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let agent = Arc::clone(&agent);
                async move |request: v2::CancelToolCallRequest,
                            responder: Responder<v2::CancelToolCallResponse>,
                            cx: V2ConnectionTo<Client>| {
                    let agent = Arc::clone(&agent);
                    let responder = errors::session_scoped(responder, &request.session_id);
                    spawn_response(&cx, responder, async move {
                        let session_id = bridge::<_, v1::SessionId>(&request.session_id)?;
                        let tool_call = request.tool_call_id.0.to_string();
                        agent
                            .stop_background(&session_id, |work| {
                                background::tool_call(work, &tool_call)
                            })
                            .await?;
                        Ok(v2::CancelToolCallResponse::new())
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
                    let session_id = bridge::<_, v1::SessionId>(&notification.session_id)?;
                    // A session not open here may be a child the host was
                    // told of, whose cancel stops that subagent.
                    if !agent.is_open(&session_id) {
                        let child = session_id.0.to_string();
                        let stopped = agent
                            .stop_any_background(|work| background::subagent(work, &child))
                            .await;
                        if stopped {
                            return Ok(());
                        }
                    }
                    agent.cancel(&session_id).await;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_close({
            let agent = Arc::clone(&agent);
            async move |_cx| {
                agent.close_all().await;
                Ok(())
            }
        })
}

/// `session/resume`, replaying history through `replay` first when asked
/// and marking its end with `replay_complete`. The session's live updates
/// and requests wait until the response: the session's pump holds them
/// behind the attach's replay (`sessions::Started::open`). Before the
/// response, v2 requires a current-state snapshot, with an announcement
/// before it of each piece of background work it lists, and asks for the
/// session's current info and usage, which the replay leaves out.
async fn resume(
    agent: &Adapter,
    request: v2::ResumeSessionRequest,
    responder: Responder<v2::ResumeSessionResponse>,
    replay: Arc<V2Port>,
    port: &V2Port,
) -> Result<(), AcpError> {
    let session_id = request.session_id.clone();
    let reattach = match requests::resume_session_request(&request) {
        Ok(reattach) => reattach,
        Err(error) => return responder.respond_with_error(error),
    };
    let replayed = matches!(reattach, Reattach::Replay(_));
    let started = async {
        let started = match reattach {
            Reattach::Replay(request) => {
                agent.load_session(&request, replay, IfOpen::Resync).await?
            }
            Reattach::Resume(request) => agent.resume_session(&request, IfOpen::Resync).await?,
        };
        let setup = requests::setup(&started.options, agent.toggles())?;
        Ok::<_, AcpError>((setup, started))
    };
    let (setup, started): (_, Started) = match started.await {
        Ok(started) => started,
        Err(error) => return responder.respond_with_error(error),
    };
    if replayed {
        port.send_update(
            &session_id,
            v2::SessionUpdate::ReplayComplete(v2::ReplayComplete::new()),
        )?;
    }
    for update in current_state(&started) {
        port.send_update(&session_id, update)?;
    }
    responder.respond(
        v2::ResumeSessionResponse::new()
            .config_options(setup.config_options)
            .available_commands(setup.available_commands),
    )?;
    started.open();
    Ok(())
}

/// What a resume reports of the session's current state, in order: each
/// piece of background work, the `state_update` that lists it, the
/// session's info, and its last usage. A session without a title yet
/// leaves the host's as it is.
fn current_state(started: &Started) -> Vec<v2::SessionUpdate> {
    let super::sessions::Current { title, usage } = &started.current;
    let mut updates = background::announcements(&started.background);
    updates.push(state(started));
    let mut info = v2::SessionInfoUpdate::new();
    if let Some(title) = title {
        info = info.title(title.clone());
    }
    updates.push(v2::SessionUpdate::SessionInfoUpdate(info));
    // Usage the host cannot read must not fail the resume.
    match usage.as_ref().map(bridge).transpose() {
        Ok(usage) => updates.extend(usage.map(v2::SessionUpdate::UsageUpdate)),
        Err(error) => tracing::warn!(%error, "could not report a session's usage over ACP v2"),
    }
    updates
}

/// The session's state when the attachment began: a turn another client
/// started may still run. The session pump follows that turn from the same
/// attach state (`sessions::pump::Following::Announced`) and reports its end
/// through `AcpClientPort::attached_turn_ended`.
fn state(started: &Started) -> v2::SessionUpdate {
    background::state(Foreground::of(started.state), &started.background)
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
