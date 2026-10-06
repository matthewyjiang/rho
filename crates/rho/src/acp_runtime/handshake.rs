//! Bounded setup steps. Prompt turns themselves have no runtime time limit.

use super::policy::AcpAgentPolicy;
use agent_client_protocol::{
    schema::{
        v1::{
            AuthenticateRequest, InitializeRequest, SessionConfigKind, SessionConfigOption,
            SessionConfigSelectOptions, SessionConfigValueId, SetSessionConfigOptionRequest,
            SetSessionModeRequest,
        },
        ProtocolVersion,
    },
    ActiveSession, Agent, ConnectionTo, Error,
};
use std::{future::Future, sync::Mutex, time::Duration};

// Receipt: Cursor ACP spike n=14: initialize 420–503 ms, session/new
// 2411–3381 ms. Ten seconds is approximately 3x the worst cold handshake.
pub(super) const HANDSHAKE_STEP_BUDGET: Duration = Duration::from_secs(10);

pub(super) fn error(message: impl Into<String>) -> Error {
    let mut error = Error::internal_error();
    error.message = message.into();
    error
}

pub(super) async fn bounded<T>(
    step: &str,
    future: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    tokio::time::timeout(HANDSHAKE_STEP_BUDGET, future)
        .await
        .map_err(|_| error(format!("acp: {step} exceeded handshake step budget 10 s")))?
        .map_err(|source| error(format!("acp: {step}: {source}")))
}

pub(super) async fn initialize<P: AcpAgentPolicy>(
    cx: &ConnectionTo<Agent>,
    policy: &Mutex<P>,
) -> Result<(), Error> {
    // Constructor defaults advertise no filesystem, terminal, or subagent
    // support. Do not enable capabilities we cannot fulfill.
    let response = bounded(
        "initialize",
        cx.send_request(InitializeRequest::new(ProtocolVersion::V1))
            .block_task(),
    )
    .await?;
    if response.protocol_version != ProtocolVersion::V1 {
        return Err(error(format!(
            "acp: initialize: unsupported protocol version {}",
            response.protocol_version
        )));
    }
    let method = policy
        .lock()
        .expect("ACP policy")
        .auth_method(&response.auth_methods);
    if let Some(method) = method {
        bounded(
            "authenticate",
            cx.send_request(AuthenticateRequest::new(method))
                .block_task(),
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn require_mode<P: AcpAgentPolicy>(
    session: &ActiveSession<'_, Agent>,
    policy: &Mutex<P>,
) -> Result<(), Error> {
    let mode = policy.lock().expect("ACP policy").session_mode();
    if let Some(mode) = mode {
        if !session.modes().is_some_and(|modes| {
            modes
                .available_modes
                .iter()
                .any(|available| available.id == mode)
        }) {
            return Err(error(format!(
                "acp: session/set_mode: required mode `{mode}` is not advertised"
            )));
        }
        bounded(
            &format!("session/set_mode `{mode}`"),
            session
                .connection()
                .send_request(SetSessionModeRequest::new(
                    session.session_id().clone(),
                    mode,
                ))
                .block_task(),
        )
        .await?;
    }
    Ok(())
}

/// Set each policy-required select option, after checking the agent
/// advertised it with that value. Already-current values are not re-sent.
/// Each response carries the full option set (one change can move others),
/// so later choices are checked against the latest one.
pub(super) async fn require_config<P: AcpAgentPolicy>(
    session: &ActiveSession<'_, Agent>,
    policy: &Mutex<P>,
) -> Result<(), Error> {
    let choices = policy.lock().expect("ACP policy").session_config();
    let mut options = session.config_options().unwrap_or_default().to_vec();
    for choice in choices {
        let step = format!("session/set_config_option `{}`", choice.id);
        let Some((current, values)) = options
            .iter()
            .find(|option| option.id == choice.id)
            .and_then(select_state)
        else {
            return Err(error(format!(
                "acp: {step}: the agent does not advertise select option `{}`",
                choice.id
            )));
        };
        if !values.contains(&&choice.value) {
            let available = values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(error(format!(
                "acp: {step}: value `{}` is not advertised (available: {available})",
                choice.value
            )));
        }
        if *current == choice.value {
            continue;
        }
        options = bounded(
            &step,
            session
                .connection()
                .send_request(SetSessionConfigOptionRequest::new(
                    session.session_id().clone(),
                    choice.id,
                    choice.value,
                ))
                .block_task(),
        )
        .await?
        .config_options;
    }
    Ok(())
}

/// Current value and every offered value of a select option; `None` for
/// other (including future) option kinds.
fn select_state(
    option: &SessionConfigOption,
) -> Option<(&SessionConfigValueId, Vec<&SessionConfigValueId>)> {
    let SessionConfigKind::Select(select) = &option.kind else {
        return None;
    };
    let values = match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => {
            options.iter().map(|option| &option.value).collect()
        }
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| &group.options)
            .map(|option| &option.value)
            .collect(),
        _ => return None,
    };
    Some((&select.current_value, values))
}
