//! What a session offers its host to show and change, as `rho-protocol`'s
//! [`wire::SessionOptions`]: its settings, with the choices it has for each
//! (model, reasoning level, fast mode, permission mode), and the commands a
//! prompt may run.
//!
//! The daemon sends them as they are, and the TUI client reads them (#1667).
//! ACP renders them as config options and `available_commands_update`
//! (`acp/config_options.rs`, `thought_level.rs`, `fast_mode.rs`,
//! `commands.rs`), renderers the ACP adapter keeps once it reads these from
//! the daemon (#1670). Each host parses its own change requests and applies
//! them through the [`SessionRuntime`]'s settings cores
//! (`set_permission_mode`, `switch_model`, `set_reasoning_level`,
//! `set_fast_mode`), then rebuilds these: a model switch adds or drops the
//! per-model reasoning levels and fast mode.

use rho_protocol as wire;
use rho_providers::{
    credentials::available_auth_modes, model::catalog, providers::fast_mode::supports_fast_mode,
};
use rho_sdk::ReasoningLevel;

use crate::{
    app::{
        session_commands::{CommandScope, CommandSet},
        session_runtime::{model_scoped_config, CurrentModel, SessionRuntime},
    },
    config::Config,
    credential_store::AppCredentialStore,
    permission::PermissionMode,
};

mod commands;
mod models;
mod reasoning;

pub(crate) use commands::available_commands;
pub(crate) use models::resolve_model;
pub(crate) use reasoning::reasoning_from_wire;

/// `runtime`'s options now: the choices `process_config` and the credential
/// store give it, and the commands of `commands` its prompts may run.
pub(crate) fn of(
    runtime: &SessionRuntime,
    process_config: &Config,
    commands: CommandSet,
) -> wire::SessionOptions {
    let home = crate::paths::home_dir();
    let scope = CommandScope::of(runtime, home.as_deref(), process_config);
    build(
        &runtime.current_model(),
        process_config,
        runtime.reasoning_level(),
        wire::FastModeOptions {
            enabled: runtime.fast_mode(),
        },
        runtime.permission_mode(),
        available_commands(&scope, commands),
    )
}

/// The options of a session on `current` at `reasoning` in
/// `permission_mode`, offering `commands`: the models of every auth the
/// credential store can use, ordered by `process_config`'s favorites, and
/// `current`'s reasoning levels. `fast_mode` is the session's fast-mode
/// preference, reported only when `current` has fast mode; the preference
/// stays on the session across a model switch, like the TUI's `/fast`, so a
/// switch back to a fast model shows it again.
fn build(
    current: &CurrentModel,
    process_config: &Config,
    reasoning: ReasoningLevel,
    fast_mode: wire::FastModeOptions,
    permission_mode: PermissionMode,
    commands: Vec<wire::SessionCommand>,
) -> wire::SessionOptions {
    let available_auths = available_auth_modes(&AppCredentialStore);
    wire::SessionOptions {
        model: models::model_options(
            current,
            &process_config.favorite_models,
            catalog::available_models_for_auths(&available_auths),
        ),
        reasoning: reasoning::reasoning_options(
            &model_scoped_config(current, process_config),
            reasoning,
        ),
        fast_mode: supports_fast_mode(&current.provider, &current.model, &current.auth)
            .then_some(fast_mode),
        permission_mode: wire::PermissionModeOptions {
            current: permission_mode.into(),
            available: PermissionMode::ALL.into_iter().map(Into::into).collect(),
        },
        commands,
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
