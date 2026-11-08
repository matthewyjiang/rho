//! One ACP prompt with no run artifacts, for Rho's internal agents (advisor).
//!
//! Delegated runs go through [`super::run_session`], which owns the run
//! directory and the status contract. A one-shot keeps nothing: it spawns the
//! agent, opens one session with the policy's setup, sends one prompt, and
//! streams the answer to `on_progress`. Spawn, the mode fence, cost, and
//! stop-reason handling are shared with delegated runs; stderr is kept as a
//! bounded tail for failure text.

use super::{
    driver::CANCEL_DRAIN_BUDGET,
    handshake::{self, error, HANDSHAKE_STEP_BUDGET},
    permission::choose_option,
    policy::AcpAgentPolicy,
    session::spawn_agent,
    transport::child_transport,
    turn::classify_stop,
    ExtensionAnswer,
};
use crate::{
    cli_runtime::{agent_event::render::usd_cost, StderrTail},
    session_environment::Ambient,
};
use agent_client_protocol::{
    schema::v1::{
        CancelNotification, ContentBlock, NewSessionRequest, RequestPermissionRequest,
        RequestPermissionResponse, SessionNotification, SessionUpdate,
    },
    util::MatchDispatch,
    Client, ConnectTo, Error, SessionMessage, UntypedMessage,
};
use rho_sdk::CancellationToken;
use std::{path::Path, process::Stdio, sync::Mutex};
use tokio::sync::oneshot;

/// What the agent is doing, for a live progress card.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OneShotProgress<'a> {
    Thinking,
    /// The answer so far.
    Answer(&'a str),
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct OneShotAnswer {
    pub(crate) text: String,
    /// Session spend the agent reported, in US dollars.
    pub(crate) cost_usd: Option<f64>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum OneShotError {
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

/// Spawn the policy's agent in `environment` and ask it `prompt` once, in
/// the plan's cwd.
///
/// There is no run directory, so the policy must be complete as constructed:
/// `prepare` is not called.
pub(crate) async fn run_one_shot<P: AcpAgentPolicy>(
    policy: P,
    prompt: &str,
    environment: &Ambient,
    cancellation: &CancellationToken,
    on_progress: impl FnMut(OneShotProgress<'_>) + Send,
) -> Result<OneShotAnswer, OneShotError> {
    let program = policy.label().program;
    let executable = policy
        .resolve_executable(environment)
        .map_err(OneShotError::Failed)?;
    let plan = policy.spawn_plan(None).map_err(OneShotError::Failed)?;
    let cwd = plan.cwd.clone();
    let (mut child, stdin, stdout) = spawn_agent(
        program,
        &executable,
        plan,
        Stdio::piped(),
        /*before_spawn*/ None,
        environment,
    )
    .map_err(OneShotError::Failed)?;
    let stderr = tokio::spawn(StderrTail::capture(child.stderr()));
    let outcome = converse(
        child_transport(stdin, stdout, program),
        policy,
        prompt,
        &cwd,
        cancellation,
        on_progress,
    )
    .await;
    // Agents keep serving after a turn; the answer is complete, so end it.
    child.terminate().await;
    let tail = stderr.await.map(StderrTail::finish).unwrap_or_default();
    outcome.map_err(|failure: OneShotError| match failure {
        OneShotError::Failed(detail) if !tail.trim().is_empty() => {
            OneShotError::Failed(format!("{detail}: {}", tail.trim()))
        }
        other => other,
    })
}

/// The protocol half of [`run_one_shot`], over any transport.
pub(super) async fn converse<P: AcpAgentPolicy>(
    transport: impl ConnectTo<Client> + 'static,
    policy: P,
    prompt: &str,
    cwd: &Path,
    cancellation: &CancellationToken,
    mut on_progress: impl FnMut(OneShotProgress<'_>) + Send,
) -> Result<OneShotAnswer, OneShotError> {
    let policy = Mutex::new(policy);
    let outcome = Client
        .builder()
        .name("rho")
        .connect_with(transport, async |cx| {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => return Ok(None),
                result = handshake::initialize(&cx, &policy) => { result?; }
            }
            let meta = policy.lock().expect("ACP policy").session_meta();
            let (started_tx, started_rx) = oneshot::channel();
            let run = cx
                .build_session_from(NewSessionRequest::new(cwd.to_path_buf()).meta(meta))
                .block_task()
                .run_until(async |mut session| {
                    let _ = started_tx.send(());
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => return Ok(None),
                        result = async {
                            handshake::require_mode(&session, &policy).await?;
                            handshake::require_config(&session, &policy).await
                        } => result?,
                    }
                    session.send_prompt(prompt)?;
                    let mut answer = OneShotAnswer::default();
                    loop {
                        let message = tokio::select! {
                            biased;
                            () = cancellation.cancelled() => {
                                session.connection().send_notification(CancelNotification::new(session.session_id().clone()))?;
                                // Give the agent the delegated runs' shutdown
                                // grace to stop the turn before the process ends.
                                let _ = tokio::time::timeout(CANCEL_DRAIN_BUDGET, async {
                                    while !matches!(session.read_update().await, Ok(SessionMessage::StopReason(_)) | Err(_)) {}
                                })
                                .await;
                                return Ok(None);
                            }
                            message = session.read_update() => message?,
                        };
                        match message {
                            SessionMessage::SessionMessage(dispatch) => {
                                answer_update(dispatch, &policy, &mut answer, &mut on_progress).await?;
                            }
                            SessionMessage::StopReason(stop) => {
                                let turn = classify_stop(&stop, /*cancelled_by_us*/ false);
                                return match turn.failure_detail() {
                                    None => Ok(Some(answer)),
                                    Some(detail) => Err(error(detail)),
                                };
                            }
                            _ => return Err(error("acp: unhandled session message")),
                        }
                    }
                });
            tokio::pin!(run);
            // Setup can be cancelled and is bounded like a delegated run's;
            // once the session exists, the loop above owns cancellation.
            tokio::select! {
                biased;
                () = cancellation.cancelled() => Ok(None),
                started = tokio::time::timeout(HANDSHAKE_STEP_BUDGET, started_rx) => match started {
                    Ok(_) => run.await,
                    Err(_) => Err(error("acp: session/new exceeded handshake step budget 10 s")),
                },
                result = &mut run => result,
            }
        })
        .await;
    match outcome {
        Ok(Some(answer)) => Ok(answer),
        Ok(None) => Err(OneShotError::Cancelled),
        Err(_) if cancellation.is_cancelled() => Err(OneShotError::Cancelled),
        Err(source) => Err(OneShotError::Failed(format!("acp: {source}"))),
    }
}

async fn answer_update<P: AcpAgentPolicy>(
    dispatch: agent_client_protocol::Dispatch,
    policy: &Mutex<P>,
    answer: &mut OneShotAnswer,
    on_progress: &mut (impl FnMut(OneShotProgress<'_>) + Send),
) -> Result<(), Error> {
    MatchDispatch::new(dispatch)
        .if_notification(async |notification: SessionNotification| {
            let (update, required_mode) = {
                let policy = policy.lock().expect("ACP policy");
                (
                    policy.normalize_update(notification.update),
                    policy.session_mode(),
                )
            };
            handshake::check_mode_update(&update, required_mode.as_ref())?;
            match update {
                SessionUpdate::AgentMessageChunk(chunk) => {
                    if let ContentBlock::Text(text) = chunk.content {
                        answer.text.push_str(&text.text);
                        on_progress(OneShotProgress::Answer(&answer.text));
                    }
                }
                SessionUpdate::AgentThoughtChunk(_) => on_progress(OneShotProgress::Thinking),
                SessionUpdate::UsageUpdate(usage) => {
                    if let Some(Ok(amount)) = usage.cost.as_ref().map(usd_cost) {
                        answer.cost_usd = Some(amount);
                    }
                }
                _ => {}
            }
            Ok(())
        })
        .await
        .if_request(async |request: RequestPermissionRequest, responder| {
            let decision = policy
                .lock()
                .expect("ACP policy")
                .decide_permission(&request);
            responder.respond(RequestPermissionResponse::new(choose_option(
                decision,
                &request.options,
            )))
        })
        .await
        .if_request(async |request: UntypedMessage, responder| {
            // No transcript to present into: only the reply is used.
            let answer = policy
                .lock()
                .expect("ACP policy")
                .answer_extension(&request.method, &request.params);
            match answer {
                Some(ExtensionAnswer { reply, .. }) => responder.respond(reply),
                None => responder.respond_with_error(Error::method_not_found()),
            }
        })
        .await
        .otherwise_ignore()
}

#[cfg(test)]
#[path = "one_shot_tests.rs"]
mod tests;
