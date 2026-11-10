//! `rho/session/configure`: one change to a session, applied between its
//! turns: a setting, through the session runtime's settings cores, the same
//! ACP's `session/set_mode` and `session/set_config_option` and the TUI's
//! controls use (#1670, #1667), or context added for the model's next turn.
//!
//! A change sent while a turn is under way, waiting for input included,
//! waits for the turn to end, then applies and answers, as the TUI and ACP
//! queue settings behind a turn (#1685). A change and a prompt's turn each
//! join the queue on the session's runtime under the turn lock, so they
//! take it in the order they started under that lock. The daemon's stop
//! answers a change still queued `DaemonStopping`. A change applies even
//! when its client disconnects meanwhile: the protocol server runs every
//! request it read to completion.
//!
//! Only a loaded session takes a change: a client attaches first, which
//! loads it. A change that applies is kept with the session's file, so a
//! session the daemon unloads while idle reopens with it.

use rho_protocol::{
    self as wire, ErrorCode, RpcError, SessionConfigureParams, SessionConfigureResponse,
};
use rho_providers::model::catalog::ModelSelection;
use rho_sdk::{model::Message, ReasoningLevel};

use super::{being_deleted, options::available_auths, queued::Queued, stopping, Held, Sessions};
use crate::{
    app::{
        session_options::{reasoning_from_wire, resolve_model},
        session_runtime::{SessionRuntime, SettingChangeError},
    },
    config::Config,
    permission::PermissionMode,
};

/// A change this daemon knows how to apply.
enum Change {
    Model(wire::ModelRef),
    Reasoning(ReasoningLevel),
    FastMode { enabled: bool },
    PermissionMode(PermissionMode),
    Context { text: String, display: String },
}

impl Sessions {
    /// Applies `params`' change to its loaded session, once the turn under
    /// way and the changes queued before it are done, and answers with the
    /// session's options.
    pub(in crate::app::daemon_host) async fn configure(
        &self,
        params: SessionConfigureParams,
    ) -> Result<SessionConfigureResponse, RpcError> {
        self.refuse_when_stopping()?;
        if let wire::SessionChange::Goal { action } = params.change {
            return self.configure_goal(params.session, action).await;
        }
        let change = Change::parse(params.change)?;
        let not_loaded = || {
            RpcError::new(
                ErrorCode::SessionNotLoaded,
                "the daemon has not loaded the session; attach to it first",
            )
        };
        let session = self
            .loaded_session(&params.session)
            .await?
            .ok_or_else(not_loaded)?;
        let (held, queued) = {
            // `None` once the idle unload took the session after the lookup.
            let mut turn = session.enter().ok_or_else(not_loaded)?;
            let held = Held::change(&session, &mut turn).ok_or_else(being_deleted)?;
            // Queued on the runtime under the turn lock a prompt starts its
            // turn under, as the change counts: a turn started before this
            // runs first, even one not holding the runtime yet, and a prompt
            // that sees the count starts a turn that queues behind it.
            (held, Queued::join(&session.runtime))
        };
        // Behind the turn under way and the changes queued before this one.
        // The stop first: a turn that outlives the stop's drain must not
        // keep this request, and so the daemon, from exiting.
        let mut runtime = tokio::select! {
            biased;
            () = self.stopping.cancelled() => return Err(stopping()),
            runtime = queued.acquire() => runtime,
        };
        // Only the daemon's stop takes the runtime of a session in use.
        let Some(runtime) = runtime.as_mut() else {
            return Err(stopping());
        };
        // Read under the runtime's lock, which a credential write's refresh
        // waits for (see `Sessions::refresh_all_options`): a refresh queued
        // before this ran first, and one queued after runs once this is
        // done, so the options this publishes are never older than one.
        let auths = available_auths(runtime.credentials()).await;
        let applied = change
            .apply(runtime, &session.config, &auths)
            .await
            .and_then(|()| {
                runtime.keep_settings().map_err(|error| {
                    RpcError::new(
                        ErrorCode::InternalError,
                        format!("could not save the session's settings: {error:#}"),
                    )
                })
            });
        // What the change saved joins the transcript a reset replays; no
        // turn's end would move it there before the next prompt.
        super::turn::save_transcript(&session, runtime);
        // Even after a failure: a model switch can fail partway, after it
        // changed what the session reports.
        let options = session.refresh_options(runtime, &auths);
        drop(held);
        applied.map(|()| SessionConfigureResponse { options })
    }
}

impl Change {
    fn parse(change: wire::SessionChange) -> Result<Self, RpcError> {
        match change {
            wire::SessionChange::Model { model } => Ok(Self::Model(model)),
            wire::SessionChange::Reasoning { level } => reasoning_from_wire(level)
                .map(Self::Reasoning)
                .ok_or_else(|| invalid("unknown reasoning level")),
            wire::SessionChange::FastMode { enabled } => Ok(Self::FastMode { enabled }),
            wire::SessionChange::PermissionMode { mode } => PermissionMode::try_from(mode)
                .map(Self::PermissionMode)
                .map_err(|_| invalid("unknown permission mode")),
            wire::SessionChange::Context { text, .. } if text.trim().is_empty() => {
                Err(invalid("context to add must not be empty"))
            }
            wire::SessionChange::Context { text, display } => Ok(Self::Context { text, display }),
            // A change of a later protocol minor fails to decode before it
            // gets here; the arm keeps the match open to new variants.
            _ => Err(invalid("unknown change")),
        }
    }

    async fn apply(
        self,
        runtime: &mut SessionRuntime,
        config: &Config,
        auths: &[String],
    ) -> Result<(), RpcError> {
        let applied = match self {
            Self::Model(model) => {
                let selection = model_selection(runtime, &model, auths)?;
                runtime.switch_model(&selection, config).await
            }
            Self::Reasoning(level) => runtime.set_reasoning_level(level, config),
            Self::FastMode { enabled } => runtime.set_fast_mode(enabled),
            Self::PermissionMode(mode) => runtime.set_permission_mode(mode, config).await,
            Self::Context { text, display } => runtime
                .append_context(text, Message::user_text(display), [])
                .map(|_| ())
                .map_err(SettingChangeError::Apply),
        };
        applied.map_err(setting_error)
    }
}

/// The selection `model` names for `runtime`'s session, with an auth in
/// `auths`.
fn model_selection(
    runtime: &SessionRuntime,
    model: &wire::ModelRef,
    auths: &[String],
) -> Result<ModelSelection, RpcError> {
    resolve_model(
        &model.provider,
        &model.model,
        &runtime.current_model(),
        auths,
    )
    .map_err(|refused| invalid(refused.to_string()))
}

/// The error a refused change answers; the session keeps its old setting.
fn setting_error(error: SettingChangeError) -> RpcError {
    match error {
        SettingChangeError::Unsupported(message) => invalid(message),
        // No turn runs while the change holds the runtime.
        SettingChangeError::Busy => RpcError::new(
            ErrorCode::InternalError,
            "could not change the session's settings: a turn ran while the change held the session's runtime, a daemon bug",
        ),
        SettingChangeError::Apply(error) => RpcError::new(
            ErrorCode::InternalError,
            format!("could not change the session's settings: {error:#}"),
        ),
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}

#[cfg(test)]
#[path = "configure_tests.rs"]
mod tests;
