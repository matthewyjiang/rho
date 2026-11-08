//! `rho/session/configure`: one change to a session's settings between its
//! turns, applied through the session runtime's settings cores, the same
//! ACP's `session/set_mode` and `session/set_config_option` and the TUI's
//! controls use (#1670, #1667).
//!
//! The daemon refuses a change while a turn is under way, waiting for input
//! included, rather than queue it behind the turn as ACP does: a daemon
//! turn can wait on a question for as long as nobody answers, and a client
//! should learn at once that its change did not apply. A prompt that
//! arrives while a change applies is refused the same way.
//!
//! Only a loaded session takes a change: a client attaches first, which
//! loads it. A change that applies is kept with the session's file, so a
//! session the daemon unloads while idle reopens with it.

use std::sync::Arc;

use rho_protocol::{
    self as wire, ErrorCode, RpcError, SessionConfigureParams, SessionConfigureResponse,
};
use rho_providers::{credentials::available_auth_modes, model::catalog::ModelSelection};
use rho_sdk::ReasoningLevel;

use super::{stopping, LoadedSession, Sessions, Settings};
use crate::{
    app::{
        session_options::{reasoning_from_wire, resolve_model},
        session_runtime::{SessionRuntime, SettingChangeError},
    },
    config::Config,
    credential_store::AppCredentialStore,
    permission::PermissionMode,
};

const TURN_UNDER_WAY: &str =
    "the session has a turn in progress; change its settings once the turn ends";
const CHANGE_APPLYING: &str =
    "another change to the session's settings is applying; send this one once it has";

/// A change this daemon knows how to apply.
enum Change {
    Model(wire::ModelRef),
    Reasoning(ReasoningLevel),
    FastMode { enabled: bool },
    PermissionMode(PermissionMode),
}

impl Sessions {
    /// Applies `params`' change to its loaded session and answers with the
    /// session's options.
    pub(in crate::app::daemon_host) async fn configure(
        &self,
        params: SessionConfigureParams,
    ) -> Result<SessionConfigureResponse, RpcError> {
        self.refuse_when_stopping()?;
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
        let changing = {
            // `None` once the idle unload took the session after the lookup.
            let mut turn = session.enter().ok_or_else(not_loaded)?;
            // Under the turn lock a prompt starts its turn under, so this
            // sees every turn started before it, and a prompt after it sees
            // the change applying.
            if self.jobs.turn_in_progress(&session.job) {
                return Err(busy(TURN_UNDER_WAY));
            }
            match turn.settings {
                Settings::Changing => return Err(busy(CHANGE_APPLYING)),
                Settings::Settled => turn.settings = Settings::Changing,
            }
            Changing(Arc::clone(&session))
        };
        // With no turn under way and none starting, only a turn that just
        // recorded its end or the daemon's stop holds the runtime, each
        // briefly.
        let mut runtime = session.runtime.lock().await;
        // Only the daemon's stop takes the runtime of a session in use.
        let Some(runtime) = runtime.as_mut() else {
            return Err(stopping());
        };
        let applied = change.apply(runtime, &session.config).await.and_then(|()| {
            runtime.keep_settings().map_err(|error| {
                RpcError::new(
                    ErrorCode::InternalError,
                    format!("could not save the session's settings: {error:#}"),
                )
            })
        });
        // Even after a failure: a model switch can fail partway, after it
        // changed what the session reports.
        let options = session.refresh_options(runtime);
        drop(changing);
        applied.map(|()| SessionConfigureResponse { options })
    }
}

/// Marks a session's settings settled again when dropped, so a prompt can
/// start, whether the change applied, failed, or its request was dropped.
struct Changing(Arc<LoadedSession>);

impl Drop for Changing {
    fn drop(&mut self) {
        self.0.turn().settings = Settings::Settled;
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
            // A change of a later protocol minor fails to decode before it
            // gets here; the arm keeps the match open to new variants.
            _ => Err(invalid("unknown change")),
        }
    }

    async fn apply(self, runtime: &mut SessionRuntime, config: &Config) -> Result<(), RpcError> {
        let applied = match self {
            Self::Model(model) => {
                let selection = model_selection(runtime, &model)?;
                runtime.switch_model(&selection, config).await
            }
            Self::Reasoning(level) => runtime.set_reasoning_level(level, config),
            Self::FastMode { enabled } => runtime.set_fast_mode(enabled),
            Self::PermissionMode(mode) => runtime.set_permission_mode(mode, config).await,
        };
        applied.map_err(setting_error)
    }
}

/// The selection `model` names for `runtime`'s session, with an auth the
/// credential store can use.
fn model_selection(
    runtime: &SessionRuntime,
    model: &wire::ModelRef,
) -> Result<ModelSelection, RpcError> {
    resolve_model(
        &model.provider,
        &model.model,
        &runtime.current_model(),
        &available_auth_modes(&AppCredentialStore),
    )
    .map_err(|refused| invalid(refused.to_string()))
}

/// The error a refused change answers; the session keeps its old setting.
fn setting_error(error: SettingChangeError) -> RpcError {
    match error {
        SettingChangeError::Unsupported(message) => invalid(message),
        SettingChangeError::Busy => busy(TURN_UNDER_WAY),
        SettingChangeError::Apply(error) => RpcError::new(
            ErrorCode::InternalError,
            format!("could not change the session's settings: {error:#}"),
        ),
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::InvalidParams, message)
}

fn busy(message: &str) -> RpcError {
    RpcError::new(ErrorCode::SessionBusy, message)
}
