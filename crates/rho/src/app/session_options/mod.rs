//! What a session offers its host to show and change, as `rho-protocol`'s
//! [`wire::SessionOptions`]: its settings, with the choices it has for each
//! (model, reasoning level, fast mode, permission mode), and the commands a
//! prompt may run.
//!
//! The daemon sends them as they are, and the TUI client reads them (#1667).
//! The ACP adapter renders them as config options and
//! `available_commands_update` (`acp/options.rs`). Each host parses its own
//! change requests and applies them through the [`SessionRuntime`]'s
//! settings cores (`set_permission_mode`, `switch_model`,
//! `set_reasoning_level`, `set_fast_mode`), then rebuilds these: a model
//! switch adds or drops the per-model reasoning levels and fast mode.

use rho_protocol as wire;
use rho_providers::{model::catalog, providers::fast_mode::supports_fast_mode};
use rho_sdk::ReasoningLevel;

use crate::{
    app::{
        session_commands::{CommandScope, CommandSet},
        session_runtime::{model_scoped_config, CurrentModel, SessionRuntime},
    },
    config::Config,
    permission::PermissionMode,
};

mod commands;
mod draft;
mod models;
mod reasoning;

pub(crate) use commands::{available_commands, drafted_commands};
pub(crate) use draft::{
    config_for_new_session, read_config_file, DraftOptions, LaunchFlag, SettingOrigin,
    SettingOrigins,
};
pub(crate) use models::{resolve_model, AuthPick, ModelIds, OfferedModels};
pub(crate) use reasoning::{reasoning_from_wire, reasoning_to_wire};

/// `runtime`'s options now: the choices `process_config` gives it, the
/// models of `available_auths` (the credential store's
/// `available_auth_modes`, which a host reads where blocking is allowed:
/// it may wait on the OS keyring), and the commands of `commands` its
/// prompts may run.
pub(crate) fn of(
    runtime: &SessionRuntime,
    process_config: &Config,
    available_auths: &[String],
    commands: CommandSet,
) -> wire::SessionOptions {
    let home = crate::paths::home_dir();
    let scope = CommandScope::of(runtime, home.as_deref(), process_config);
    let mut options = build(
        &runtime.current_model(),
        process_config,
        available_auths,
        runtime.reasoning_level(),
        wire::FastModeOptions {
            enabled: runtime.fast_mode(),
        },
        runtime.permission_mode(),
        available_commands(&scope, commands),
    );
    options.added_dirs = runtime.added_dirs().as_slice().to_vec();
    options
}

/// The options of a session on `current` at `reasoning` in
/// `permission_mode`, offering `commands`: the models of every auth in
/// `available_auths`, ordered by `process_config`'s favorites, and
/// `current`'s reasoning levels. `fast_mode` is the session's fast-mode
/// preference, reported only when `current` has fast mode; the preference
/// stays on the session across a model switch, like the TUI's `/fast`, so a
/// switch back to a fast model shows it again.
fn build(
    current: &CurrentModel,
    process_config: &Config,
    available_auths: &[String],
    reasoning: ReasoningLevel,
    fast_mode: wire::FastModeOptions,
    permission_mode: PermissionMode,
    commands: Vec<wire::SessionCommand>,
) -> wire::SessionOptions {
    wire::SessionOptions {
        model: models::model_options(
            current,
            &process_config.favorite_models,
            catalog::available_models_for_auths(available_auths),
        ),
        reasoning: reasoning::reasoning_options(
            &model_scoped_config(current, process_config),
            reasoning,
        ),
        fast_mode: supports_fast_mode(&current.provider, &current.model, &current.auth)
            .then_some(fast_mode),
        permission_mode: wire::PermissionModeOptions {
            current: permission_mode.into(),
            // Auto runs only with a classifier model, so a host offers it
            // only then, or while the session already runs it.
            available: PermissionMode::ALL
                .into_iter()
                .filter(|mode| {
                    *mode != PermissionMode::Auto
                        || permission_mode == PermissionMode::Auto
                        || process_config
                            .internal_agent_model(crate::agent::PERMISSION_CLASSIFIER_AGENT_ID)
                            .is_some()
                })
                .map(Into::into)
                .collect(),
        },
        commands,
        added_dirs: Vec::new(),
    }
}

/// The auths `credentials` can use, whose models a session may switch to,
/// read off the async workers: a locked OS keyring blocks its reader, as
/// `daemon_host/auth.rs` notes.
pub(crate) async fn available_auths(
    credentials: std::sync::Arc<dyn rho_providers::credentials::CredentialStore>,
) -> Vec<String> {
    match tokio::task::spawn_blocking(move || {
        rho_providers::credentials::available_auth_modes(credentials.as_ref())
    })
    .await
    {
        Ok(auths) => auths,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        // Cancelled before it ran: the runtime is shutting down, and no
        // client reads options again.
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
