//! `rho/model/list` and `rho/model/refresh`: the models a client may pick
//! for a model setting outside a session, and fetching the provider model
//! lists they come from ([`crate::app::model_refresh`]).
//!
//! The daemon owns those lists, so every client reads the same ones: it
//! fetches them on `rho/model/refresh`, and a provider's once a credential
//! for it is saved (`auth.rs`, `logins.rs`, [`refresh_after_sign_in`]).
//! A listing client's credentials decide which providers its list holds, so
//! each subscriber gets its own list, built again from its environment
//! whenever [`Sessions::models_changed`] signals, and sent when it differs.

use std::sync::Arc;

use rho_protocol::{
    ClaudeCliModel, ErrorCode, ModelList, ModelListParams, ModelListRefresh, ModelRefreshParams,
    ModelRefreshResponse, ModelRefreshTarget, ModelWatch, RpcError,
};
use rho_providers::{credentials::CredentialStore, model::catalog, provider};
use tokio::sync::watch;

use super::{
    auth::{client_credentials, config_and_auths},
    sessions::Sessions,
};
use crate::{
    app::{model_refresh, session_options::model_choices},
    claude_runtime::models::{
        CLAUDE_CODE_SOURCE_LABEL, CLAUDE_DEFAULT_MODEL_BADGE, CLAUDE_DEFAULT_MODEL_DETAIL,
        CLAUDE_MODEL_ALIASES,
    },
    config::Config,
    credential_store::AppCredentialStore,
};

/// Answers `rho/model/list` with the models `params`' client may pick now,
/// and keeps them current for it until the connection drops the watch.
pub(super) async fn watch(
    params: ModelListParams,
    sessions: &Sessions,
) -> Result<ModelWatch, RpcError> {
    let credentials = client_credentials(&params.environment)?;
    // Taken before the first list, so a change while it builds builds it
    // again.
    let changes = sessions.model_changes();
    let first = list(Arc::clone(&credentials)).await?;
    let (sender, receiver) = watch::channel(Arc::new(first));
    tokio::spawn(follow(credentials, changes, sender));
    Ok(ModelWatch::new(receiver))
}

/// Builds the list of `credentials`' client again each time `changes`
/// moves, and publishes it when it differs, until the connection dropped
/// the watch or the daemon is gone.
async fn follow(
    credentials: Arc<dyn CredentialStore>,
    mut changes: watch::Receiver<()>,
    sender: watch::Sender<Arc<ModelList>>,
) {
    loop {
        tokio::select! {
            changed = changes.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = sender.closed() => return,
        }
        match list(Arc::clone(&credentials)).await {
            Ok(next) => {
                sender.send_if_modified(|current| {
                    let differs = **current != next;
                    if differs {
                        *current = Arc::new(next);
                    }
                    differs
                });
            }
            // The client keeps the last list; the next change tries again.
            Err(error) => tracing::warn!("could not list models: {}", error.message),
        }
    }
}

/// The models `credentials`' client may pick: those of every auth it
/// has credentials for, the config's favorites first, then Claude Code's.
async fn list(credentials: Arc<dyn CredentialStore>) -> Result<ModelList, RpcError> {
    let (config, auths) = config_and_auths(credentials).await?;
    Ok(ModelList {
        models: model_choices(
            &config.favorite_models,
            catalog::available_models_for_auths(&auths),
        ),
        claude_cli: claude_cli_models(),
    })
}

/// Claude Code's models as a model setting offers them: its own default,
/// then each alias, labelled as the TUI's pickers label them.
fn claude_cli_models() -> Vec<ClaudeCliModel> {
    let label = |model: &str| provider::model_reference(CLAUDE_CODE_SOURCE_LABEL, model);
    std::iter::once(ClaudeCliModel {
        model: None,
        label: label(CLAUDE_DEFAULT_MODEL_BADGE),
        detail: CLAUDE_DEFAULT_MODEL_DETAIL.to_owned(),
    })
    .chain(CLAUDE_MODEL_ALIASES.iter().map(|alias| ClaudeCliModel {
        model: Some(alias.name.to_owned()),
        label: label(alias.name),
        detail: alias.detail.to_owned(),
    }))
    .collect()
}

/// Answers `rho/model/refresh`: fetches the lists `params` asks for with
/// its client's credentials, then every session's options and every model
/// list follow.
pub(super) async fn refresh(
    params: ModelRefreshParams,
    sessions: &Sessions,
) -> Result<ModelRefreshResponse, RpcError> {
    let ModelRefreshParams { environment, only } = params;
    let credentials = client_credentials(&environment)?;
    let (config, auths) = config_and_auths(Arc::clone(&credentials)).await?;
    let response = match only {
        Some(ModelRefreshTarget { provider, auth }) => {
            let Some(descriptor) = provider::provider_descriptor(&provider)
                .filter(|descriptor| descriptor.auth_mode(&auth).is_some())
            else {
                return Err(RpcError::new(
                    ErrorCode::InvalidParams,
                    format!("provider '{provider}' does not take auth '{auth}'"),
                ));
            };
            // As a full refresh skips it: a local server is never probed
            // before its login.
            if config.providers.awaits_endpoint_login(*descriptor) {
                return Err(RpcError::new(
                    ErrorCode::InvalidParams,
                    format!("provider '{provider}' is not logged in; log in to it first"),
                ));
            }
            ModelRefreshResponse {
                lists: model_refresh::refresh_provider(
                    &provider,
                    &auth,
                    credentials.as_ref(),
                    &config,
                )
                .await,
                catalog: None,
            }
        }
        None => model_refresh::refresh_all(credentials.as_ref(), &config, &auths).await,
    };
    sessions.models_changed().await;
    Ok(response)
}

/// Fetches the lists of `auth`'s provider with the credential a sign-in
/// just saved to `credentials`, as `config` reaches the provider. The
/// caller then tells every session ([`super::auth::credentials_changed`]).
pub(super) async fn refresh_after_sign_in(
    auth: &str,
    credentials: &dyn CredentialStore,
    config: &Config,
) -> Vec<ModelListRefresh> {
    let Some((descriptor, _)) = provider::resolve_auth_mode(auth) else {
        return Vec::new();
    };
    // The OS store's cache would still miss the credential just saved.
    AppCredentialStore::reread_external_logins();
    model_refresh::refresh_provider(descriptor.name, auth, credentials, config).await
}
