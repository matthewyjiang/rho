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
//! A permission mode change still waiting when an approved plan switches
//! the mode is dropped. The in-process TUI applies such a change at the end
//! of the turn it was sent during, so the approval, at the end of the same
//! turn, sets the mode the session ends in either way.
//!
//! Only a loaded session takes a change: a client attaches first, which
//! loads it. A change that applies is kept with the session's file, so a
//! session the daemon unloads while idle reopens with it.

use std::path::PathBuf;

use rho_protocol::{
    self as wire, ErrorCode, RpcError, SessionConfigureParams, SessionConfigureResponse,
};
use rho_providers::model::catalog::ModelSelection;
use rho_sdk::{model::Message, ReasoningLevel};

use super::{being_deleted, options::available_auths, queued::Queued, stopping, Held, Sessions};
use crate::{
    app::{
        session_options::{reasoning_from_wire, resolve_model, AuthPick},
        session_runtime::{
            AddDirError, RewindError, RewoundConversation, SelectNodeError, SessionRuntime,
            SettingChangeError,
        },
        workspace_rewind::rewind_audit_text,
    },
    config::Config,
    permission::PermissionMode,
    session::tree::NodeId,
};

/// A change this daemon knows how to apply.
enum Change {
    Model {
        model: wire::ModelRef,
        auth: Option<String>,
        setup: bool,
    },
    Reasoning(ReasoningLevel),
    FastMode {
        enabled: bool,
    },
    PermissionMode(PermissionMode),
    Context {
        text: String,
        display: String,
    },
    AgentCatalog,
    AddDirectory(PathBuf),
    ModelCatalog,
    SelectNode(NodeId),
    RewindWorkspace(NodeId),
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
        // A model change that waits on a `/compact`, which the turn under
        // way or last done must be.
        let after_compaction = matches!(
            params.change,
            wire::SessionChange::ModelAfterCompaction { .. }
        );
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
        let (held, queued, plan_switches, prompt) = {
            // `None` once the idle unload took the session after the lookup.
            let mut turn = session.enter().ok_or_else(not_loaded)?;
            let held = Held::change(&session, &mut turn).ok_or_else(being_deleted)?;
            // Queued on the runtime under the turn lock a prompt starts its
            // turn under, as the change counts: a turn started before this
            // runs first, even one not holding the runtime yet, and a prompt
            // that sees the count starts a turn that queues behind it.
            (
                held,
                Queued::join(&session.runtime),
                turn.plan_switches,
                turn.prompts,
            )
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
        let superseded = {
            let turn = session.turn();
            match change {
                Change::PermissionMode(_) => turn.plan_switches != plan_switches,
                // Dropped unless that turn compacted.
                _ if after_compaction => turn.compacted_prompt != Some(prompt),
                _ => false,
            }
        };
        let applied = if superseded {
            Ok(Applied::Superseded)
        } else {
            match change {
                Change::PermissionMode(_) => {
                    let config = session.config_for_permission_change().await;
                    let applied = change.apply(runtime, &config, &auths).await;
                    // Only a mode the session took keeps the models it read.
                    if applied.is_ok() {
                        session.keep_config(config);
                    }
                    applied
                }
                _ => change.apply(runtime, &session.config(), &auths).await,
            }
        };
        let applied = match applied {
            // The session keeps a setting with its file from its first
            // change on.
            Ok(Applied::Setting { notice }) => {
                session
                    .feed
                    .publish(notice.map(|text| wire::SessionEvent::Notice { text }));
                runtime.keep_settings().map_err(|error| {
                    RpcError::new(
                        ErrorCode::InternalError,
                        format!("could not save the session's settings: {error:#}"),
                    )
                })
            }
            // What it saved joins the transcript a reset replays; no turn's
            // end would move it there before the next prompt. The turn
            // before it stops replaying its end, as when a turn follows.
            Ok(Applied::Rows { notice }) => {
                // Told live first; a later reset replays the saved rows.
                session
                    .feed
                    .publish(notice.map(|text| wire::SessionEvent::Notice { text }));
                super::turn::save_transcript(&session, runtime);
                Ok(())
            }
            // Every client replays the conversation as it now stands.
            Ok(Applied::Restarted { notice }) => {
                super::turn::restart_transcript(&session, runtime).await;
                // After the restart, so the clients it detached replay it.
                session
                    .feed
                    .publish(notice.map(|text| wire::SessionEvent::Notice { text }));
                Ok(())
            }
            Ok(Applied::SharedOptions) => {
                // Each session's, once its runtime is free; this one's
                // also follow below.
                self.refresh_all_options().await;
                Ok(())
            }
            Ok(Applied::Superseded) => Ok(()),
            Err(error) => Err(error),
        };
        // Even after a failure: a model switch can fail partway, after it
        // changed what the session reports.
        let options = session.refresh_options(runtime, &auths);
        drop(held);
        applied.map(|()| SessionConfigureResponse { options })
    }
}

/// What applying a change left to do.
enum Applied {
    /// A setting changed, which the session now keeps, with a notice to
    /// tell the session's clients now.
    Setting { notice: Option<String> },
    /// Rows may have been saved, which the transcript takes, with a notice
    /// to tell the session's clients now.
    Rows { notice: Option<String> },
    /// The conversation moved to another node of the session's tree, with
    /// a notice to tell the session's clients once they replay it.
    Restarted { notice: Option<String> },
    /// What every loaded session's options show was read again.
    SharedOptions,
    /// Nothing: a plan approved after the change was sent set the
    /// permission mode it asked for, or the `/compact` a model change
    /// waited on did not reduce the conversation.
    Superseded,
}

impl Change {
    fn parse(change: wire::SessionChange) -> Result<Self, RpcError> {
        match change {
            wire::SessionChange::Model { model, auth, setup } => {
                Ok(Self::Model { model, auth, setup })
            }
            wire::SessionChange::ModelAfterCompaction { model, auth } => Ok(Self::Model {
                model,
                auth,
                setup: false,
            }),
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
            wire::SessionChange::AgentCatalog => Ok(Self::AgentCatalog),
            wire::SessionChange::AddDirectory { path } if !path.is_absolute() => {
                Err(invalid("the directory to add must be an absolute path"))
            }
            wire::SessionChange::AddDirectory { path } => Ok(Self::AddDirectory(path)),
            wire::SessionChange::ModelCatalog => Ok(Self::ModelCatalog),
            wire::SessionChange::SelectNode { node } => NodeId::from_string(node)
                .map(Self::SelectNode)
                .map_err(|error| invalid(format!("{error:#}"))),
            wire::SessionChange::RewindWorkspace { checkpoint } => NodeId::from_string(checkpoint)
                .map(Self::RewindWorkspace)
                .map_err(|error| invalid(format!("{error:#}"))),
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
    ) -> Result<Applied, RpcError> {
        let applied = match self {
            Self::Context { text, display } => {
                return runtime
                    .append_context(text, Message::user_text(display), [])
                    .map(|_| Applied::Rows { notice: None })
                    .map_err(|error| {
                        RpcError::new(
                            ErrorCode::InternalError,
                            format!("could not save the context: {error:#}"),
                        )
                    })
            }
            Self::AgentCatalog => {
                return runtime
                    .sync_agent_catalog()
                    .map(|notice| Applied::Rows {
                        notice: notice.map(str::to_owned),
                    })
                    .map_err(|error| {
                        RpcError::new(
                            ErrorCode::InternalError,
                            format!("could not read the agent definitions: {error:#}"),
                        )
                    })
            }
            Self::AddDirectory(path) => {
                // Canonical, as the session compares the roots it has.
                let dir = crate::added_dirs::resolve(&path, runtime.cwd(), /*home*/ None)
                    .map_err(|error| invalid(format!("{error:#}")))?;
                let notice = format!("added directory {}", crate::paths::display(&dir));
                return match runtime.add_dir(dir, config).await {
                    Ok(()) => Ok(Applied::Rows {
                        notice: Some(notice),
                    }),
                    Err(error @ AddDirError::Covered { .. }) => Err(invalid(error.to_string())),
                    Err(AddDirError::Change(error)) => Err(setting_error(error)),
                };
            }
            Self::ModelCatalog => {
                // Names this process resolved, or failed to, before the
                // client rewrote the catalogs.
                rho_providers::model::forget_model_display_names();
                return Ok(Applied::SharedOptions);
            }
            Self::SelectNode(node) => {
                return match runtime.select_node(&node, config).await {
                    Ok(()) => Ok(Applied::Restarted { notice: None }),
                    Err(SelectNodeError::Change(error)) => Err(setting_error(error)),
                    Err(error @ (SelectNodeError::Unsaved | SelectNodeError::UnknownNode(_))) => {
                        Err(invalid(error.to_string()))
                    }
                };
            }
            Self::RewindWorkspace(checkpoint) => {
                let rewind = runtime
                    .rewind_workspace(&checkpoint, config)
                    .await
                    .map_err(|error| match error {
                        RewindError::Refused(error) => invalid(format!("{error:#}")),
                        RewindError::Failed(error) => RpcError::new(
                            ErrorCode::InternalError,
                            format!("could not rewind the workspace: {error:#}"),
                        ),
                    })?;
                let audit = |restored| rewind_audit_text(&rewind.audit, runtime.cwd(), restored);
                return Ok(match rewind.conversation {
                    RewoundConversation::Restored => Applied::Restarted {
                        notice: Some(audit(true)),
                    },
                    RewoundConversation::Kept => Applied::Rows {
                        notice: Some(audit(false)),
                    },
                    // The files changed all the same, which every client
                    // must learn, then why the conversation stayed.
                    RewoundConversation::Failed(error) => Applied::Rows {
                        notice: Some(format!(
                            "{}\ncould not restore the conversation: {error}",
                            audit(false)
                        )),
                    },
                });
            }
            Self::Model { model, auth, setup } => {
                let selection = model_selection(runtime, &model, auth.as_deref(), auths)?;
                let previous = runtime.current_model();
                return runtime
                    .switch_model(&selection, config)
                    .await
                    .map(|()| {
                        let current = runtime.current_model();
                        // The prompt follows the model alone: a change that
                        // kept the model has no other prompt file to report.
                        let moved = !setup
                            && (current.provider, current.model)
                                != (previous.provider, previous.model);
                        Applied::Setting {
                            notice: moved.then(|| runtime.model_prompt_notice()).flatten(),
                        }
                    })
                    .map_err(setting_error);
            }
            Self::Reasoning(level) => runtime.set_reasoning_level(level, config),
            Self::FastMode { enabled } => runtime.set_fast_mode(enabled),
            Self::PermissionMode(mode) => runtime.reapply_permission_mode(mode, config).await,
        };
        applied
            .map(|()| Applied::Setting { notice: None })
            .map_err(setting_error)
    }
}

/// The selection `model` names for `runtime`'s session, on `auth` when the
/// change names one, else with an auth in `auths`.
fn model_selection(
    runtime: &SessionRuntime,
    model: &wire::ModelRef,
    auth: Option<&str>,
    auths: &[String],
) -> Result<ModelSelection, RpcError> {
    let auth = auth.map_or(AuthPick::AnyOf(auths), AuthPick::Named);
    resolve_model(
        &model.provider,
        &model.model,
        &runtime.current_model(),
        auth,
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
