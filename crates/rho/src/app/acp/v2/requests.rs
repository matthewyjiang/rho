//! ACP v2 requests in the v1 form Rho's agent handles, and the v1 answers in
//! v2 form.
//!
//! Like [`super::updates`], shapes v2 only widened cross through
//! [`super::bridge`]; redesigned ones are mapped here.

use agent_client_protocol::schema::{v1, v2, ProtocolVersion};
use agent_client_protocol::Error as AcpError;

use super::bridge;

/// `initialize` as v1 would have negotiated it. v2 made notices, compaction
/// cards, and boolean config options baseline, so the v1 markers for them are
/// set; v2 has no client `fs` or `terminal`, so Rho keeps file access and
/// shell commands local.
pub(super) fn initialize_request(
    request: &v2::InitializeRequest,
) -> Result<v1::InitializeRequest, AcpError> {
    let capabilities = &request.capabilities;
    let auth = v1::AuthCapabilities::new().terminal(
        capabilities
            .auth
            .as_ref()
            .is_some_and(|auth| auth.terminal.is_some()),
    );
    let session = v1::ClientSessionCapabilities::new()
        .compaction(v1::CompactionCapabilities::new())
        .notices(v1::NoticeCapabilities::new())
        .config_options(
            v1::SessionConfigOptionsCapabilities::new()
                .boolean(v1::BooleanConfigOptionCapabilities::new()),
        );
    let elicitation = capabilities
        .elicitation
        .as_ref()
        .map(bridge::<_, v1::ElicitationCapabilities>)
        .transpose()?;
    Ok(v1::InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(
            v1::ClientCapabilities::new()
                .auth(auth)
                .session(session)
                .elicitation(elicitation),
        )
        .client_info(bridge::<_, v1::Implementation>(&request.info)?))
}

/// The v1 `initialize` answer in v2 form. Rho serves every baseline session
/// method, so `session` is advertised; v1's `loadSession` and its
/// list/resume/close markers have no v2 counterpart.
pub(super) fn initialize_response(
    response: v1::InitializeResponse,
) -> Result<v2::InitializeResponse, AcpError> {
    let agent = response.agent_capabilities;
    let prompt = &agent.prompt_capabilities;
    let mut prompt_v2 = v2::PromptCapabilities::new();
    if prompt.image {
        prompt_v2 = prompt_v2.image(v2::PromptImageCapabilities::new());
    }
    if prompt.audio {
        prompt_v2 = prompt_v2.audio(v2::PromptAudioCapabilities::new());
    }
    if prompt.embedded_context {
        prompt_v2 = prompt_v2.embedded_context(v2::PromptEmbeddedContextCapabilities::new());
    }
    // v1 implied stdio MCP servers; v2 asks for the marker.
    let mut mcp = v2::McpCapabilities::new().stdio(v2::McpStdioCapabilities::new());
    if agent.mcp_capabilities.http {
        mcp = mcp.http(v2::McpHttpCapabilities::new());
    }
    let sessions = &agent.session_capabilities;
    let mut session = v2::SessionCapabilities::new().prompt(prompt_v2).mcp(mcp);
    if sessions.delete.is_some() {
        session = session.delete(v2::SessionDeleteCapabilities::new());
    }
    if sessions.additional_directories.is_some() {
        session =
            session.additional_directories(v2::SessionAdditionalDirectoriesCapabilities::new());
    }
    let info = match &response.agent_info {
        Some(info) => bridge(info)?,
        None => v2::Implementation::new("rho", env!("CARGO_PKG_VERSION")),
    };
    Ok(v2::InitializeResponse::new(ProtocolVersion::V2, info)
        .capabilities(v2::AgentCapabilities::new().session(session))
        .auth_methods(
            response
                .auth_methods
                .iter()
                .map(auth_method)
                .collect::<Result<_, _>>()?,
        ))
}

/// v2 renames `id` to `methodId` and lists terminal environment variables as
/// name/value entries.
fn auth_method(method: &v1::AuthMethod) -> Result<v2::AuthMethod, AcpError> {
    Ok(match method {
        v1::AuthMethod::Agent(agent) => v2::AuthMethod::Agent(
            v2::AuthMethodAgent::new(agent.id.0.clone(), agent.name.clone())
                .description(agent.description.clone())
                .meta(agent.meta.clone()),
        ),
        v1::AuthMethod::Terminal(terminal) => {
            let mut env: Vec<_> = terminal
                .env
                .iter()
                .map(|(name, value)| v2::EnvVariable::new(name.clone(), value.clone()))
                .collect();
            env.sort_by(|a, b| a.name.cmp(&b.name));
            v2::AuthMethod::Terminal(
                v2::AuthMethodTerminal::new(terminal.id.0.clone(), terminal.name.clone())
                    .description(terminal.description.clone())
                    .args(terminal.args.clone())
                    .env(env)
                    .meta(terminal.meta.clone()),
            )
        }
        _ => return Err(AcpError::internal_error().data("no ACP v2 form for this auth method")),
    })
}

/// The lifecycle environment shared by `session/new` and `session/resume`.
struct Environment {
    cwd: std::path::PathBuf,
    additional_directories: Vec<std::path::PathBuf>,
    mcp_servers: Vec<v1::McpServer>,
}

fn environment(
    cwd: &v2::AbsolutePath,
    additional_directories: &[v2::AbsolutePath],
    mcp_servers: &[v2::McpServer],
) -> Result<Environment, AcpError> {
    Ok(Environment {
        cwd: cwd.0.clone(),
        additional_directories: additional_directories
            .iter()
            .map(|path| path.0.clone())
            .collect(),
        mcp_servers: mcp_servers
            .iter()
            .map(mcp_server)
            .collect::<Result<_, _>>()?,
    })
}

/// v2 tags stdio servers `type: "stdio"`, which v1 leaves untagged.
fn mcp_server(server: &v2::McpServer) -> Result<v1::McpServer, AcpError> {
    Ok(match server {
        v2::McpServer::Stdio(stdio) => v1::McpServer::Stdio(
            v1::McpServerStdio::new(stdio.name.clone(), stdio.command.0.clone())
                .args(stdio.args.clone())
                .env(bridge(&stdio.env)?)
                .meta(stdio.meta.clone()),
        ),
        v2::McpServer::Http(http) => v1::McpServer::Http(
            v1::McpServerHttp::new(http.name.clone(), http.url.clone())
                .headers(bridge(&http.headers)?)
                .meta(http.meta.clone()),
        ),
        _ => {
            return Err(AcpError::invalid_params()
                .data("Rho supports stdio and http MCP servers over ACP v2"))
        }
    })
}

pub(super) fn new_session_request(
    request: &v2::NewSessionRequest,
) -> Result<v1::NewSessionRequest, AcpError> {
    let env = environment(
        &request.cwd,
        &request.additional_directories,
        &request.mcp_servers,
    )?;
    Ok(v1::NewSessionRequest::new(env.cwd)
        .additional_directories(env.additional_directories)
        .mcp_servers(env.mcp_servers)
        .meta(request.meta.clone()))
}

/// How a v2 `session/resume` reattaches, by its replay cursor.
pub(super) enum Reattach {
    /// `replayFrom: start`: v1 `session/load`, which replays history.
    Replay(v1::LoadSessionRequest),
    /// No cursor: v1 `session/resume`; the client keeps its transcript.
    Resume(v1::ResumeSessionRequest),
}

pub(super) fn resume_session_request(
    request: &v2::ResumeSessionRequest,
) -> Result<Reattach, AcpError> {
    let env = environment(
        &request.cwd,
        &request.additional_directories,
        &request.mcp_servers,
    )?;
    let session_id = bridge::<_, v1::SessionId>(&request.session_id)?;
    Ok(match &request.replay_from {
        None => Reattach::Resume(
            v1::ResumeSessionRequest::new(session_id, env.cwd)
                .additional_directories(env.additional_directories)
                .mcp_servers(env.mcp_servers)
                .meta(request.meta.clone()),
        ),
        Some(v2::ReplayFrom::Start(_)) => Reattach::Replay(
            v1::LoadSessionRequest::new(session_id, env.cwd)
                .additional_directories(env.additional_directories)
                .mcp_servers(env.mcp_servers)
                .meta(request.meta.clone()),
        ),
        Some(_) => {
            return Err(AcpError::invalid_params().data("Rho replays only from the start"));
        }
    })
}

/// The config options and commands a v2 session setup response carries. v1
/// sent the commands as a notification after the response; v2 puts them in it.
pub(super) struct Setup {
    pub(super) config_options: Vec<v2::SessionConfigOption>,
    pub(super) available_commands: Vec<v2::AvailableCommand>,
}

pub(super) fn setup(
    config_options: Option<Vec<v1::SessionConfigOption>>,
    commands: v1::SessionNotification,
) -> Result<Setup, AcpError> {
    let available_commands = match commands.update {
        v1::SessionUpdate::AvailableCommandsUpdate(update) => {
            super::updates::available_commands(update.available_commands)?
        }
        _ => Vec::new(),
    };
    Ok(Setup {
        config_options: super::updates::config_options(config_options.unwrap_or_default())?,
        available_commands,
    })
}

pub(super) fn prompt_request(request: &v2::PromptRequest) -> Result<v1::PromptRequest, AcpError> {
    Ok(v1::PromptRequest::new(
        bridge::<_, v1::SessionId>(&request.session_id)?,
        bridge(&request.prompt).map_err(|_| {
            AcpError::invalid_params().data("prompt content Rho does not understand")
        })?,
    )
    .meta(request.meta.clone()))
}

pub(super) fn set_config_option_request(
    request: &v2::SetSessionConfigOptionRequest,
) -> Result<v1::SetSessionConfigOptionRequest, AcpError> {
    let value = match &request.value {
        v2::SessionConfigOptionValue::Id { value } => {
            v1::SessionConfigOptionValue::value_id(value.0.clone())
        }
        v2::SessionConfigOptionValue::Boolean { value } => {
            v1::SessionConfigOptionValue::boolean(*value)
        }
        _ => return Err(AcpError::invalid_params().data("unknown config option value type")),
    };
    Ok(v1::SetSessionConfigOptionRequest::new(
        bridge::<_, v1::SessionId>(&request.session_id)?,
        request.config_id.0.clone(),
        value,
    )
    .meta(request.meta.clone()))
}

/// v1 Rho puts the prompt's copy in the tool call's title; v2 gives it its own
/// `title` field, so the card keeps its real title. Everything else on the
/// v1 tool call is genuine card state and becomes the subject.
pub(super) fn permission_request(
    request: v1::RequestPermissionRequest,
) -> Result<v2::RequestPermissionRequest, AcpError> {
    let v1::RequestPermissionRequest {
        session_id,
        mut tool_call,
        options,
        meta,
        ..
    } = request;
    let title = tool_call
        .fields
        .title
        .take()
        .unwrap_or_else(|| "Allow this tool call?".to_owned());
    let subject = v2::RequestPermissionSubject::ToolCall(Box::new(
        v2::ToolCallPermissionSubject::new(super::updates::tool_call_update(tool_call)?),
    ));
    Ok(v2::RequestPermissionRequest::new(
        bridge::<_, v2::SessionId>(&session_id)?,
        title,
        bridge(&options)?,
    )
    .subject(subject)
    .meta(meta))
}

/// An outcome v1 cannot express is no approval.
pub(super) fn permission_response(
    response: v2::RequestPermissionResponse,
) -> v1::RequestPermissionResponse {
    let outcome = match response.outcome {
        v2::RequestPermissionOutcome::Selected(selected) => v1::RequestPermissionOutcome::Selected(
            v1::SelectedPermissionOutcome::new(selected.option_id.0.clone()),
        ),
        _ => v1::RequestPermissionOutcome::Cancelled,
    };
    v1::RequestPermissionResponse::new(outcome).meta(response.meta)
}

/// Why foreground work stopped, for the closing `idle` update. A reason v2
/// cannot name leaves the `idle` without one.
pub(super) fn stop_reason(reason: v1::StopReason) -> Option<v2::StopReason> {
    Some(match reason {
        v1::StopReason::EndTurn => v2::StopReason::EndTurn,
        v1::StopReason::MaxTokens => v2::StopReason::MaxTokens,
        v1::StopReason::MaxTurnRequests => v2::StopReason::MaxTurnRequests,
        v1::StopReason::Refusal => v2::StopReason::Refusal,
        v1::StopReason::Cancelled => v2::StopReason::Cancelled,
        _ => return None,
    })
}

/// A failure after the prompt was accepted, for the closing `idle` update.
pub(super) fn failed(error: &AcpError) -> Option<v2::StopReason> {
    let error = bridge::<_, v2::Error>(error).ok();
    Some(v2::StopReason::Error(
        v2::ErrorStopReason::new().error(error),
    ))
}

#[cfg(test)]
#[path = "requests_tests.rs"]
mod tests;
