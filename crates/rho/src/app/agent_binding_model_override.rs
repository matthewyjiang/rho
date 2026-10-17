//! Explicit delegation selections use the same catalog and alias vocabulary as
//! `/model`, without modifying the agent definition or inheriting its auth pin.

use rho_providers::{
    credentials::{self, CredentialStore},
    model::catalog,
};

use crate::config::Config;

/// New references choose an available auth profile; resumed selections must
/// validate and keep the exact profile that the child already used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelOverride<'a> {
    Reference(&'a str),
    Saved {
        provider: &'a str,
        model: &'a str,
        auth: &'a str,
    },
}

pub(super) fn bind_config(
    agent_id: &str,
    model_override: ModelOverride<'_>,
    host: &Config,
    store: &dyn CredentialStore,
) -> anyhow::Result<Config> {
    let reference = match model_override {
        ModelOverride::Reference(reference) => reference,
        ModelOverride::Saved {
            provider,
            model,
            auth,
        } => {
            return restore_config(agent_id, provider, model, auth, host, store);
        }
    };
    let resolve = || -> anyhow::Result<Config> {
        let resolved = host.model_aliases.resolve(reference)?;
        let available = credentials::available_auth_modes(store);
        let auth_context = catalog::SelectionAuthContext {
            current: Some(&host.auth),
            available: &available,
        };
        let selection = match resolved.provider.as_deref() {
            Some(provider) => catalog::resolve_model_selection_for_provider(
                provider,
                &resolved.model,
                auth_context,
            )?,
            None if resolved.alias.is_some() => catalog::resolve_model_selection_for_provider(
                &host.provider,
                &resolved.model,
                auth_context,
            )?,
            None => catalog::resolve_model_selection_for_auths(
                &resolved.model,
                &host.provider,
                &host.auth,
                &available,
            )?,
        };
        if !credentials::auth_has_credentials(store, &selection.auth)? {
            anyhow::bail!(
                "no credentials available for {}/{} with auth '{}'; log in to the selected provider",
                selection.provider, selection.model, selection.auth
            );
        }
        let mut config = host.clone();
        config.provider = selection.provider;
        config.model = selection.model;
        config.auth = selection.auth;
        config.model_alias = resolved.alias;
        Ok(config)
    };
    resolve().map_err(|error| {
        anyhow::anyhow!("agent '{agent_id}' model override '{reference}': {error}")
    })
}

/// Constrain catalog validation to the saved auth, including keyless `none`.
/// An optional key appearing later must not make a valid session unloadable.
fn restore_config(
    agent_id: &str,
    provider: &str,
    model: &str,
    auth: &str,
    host: &Config,
    store: &dyn CredentialStore,
) -> anyhow::Result<Config> {
    rho_providers::provider::resolve_profile_exact(provider, auth)?;
    let available = [auth.to_owned()];
    let selection = catalog::resolve_model_selection_for_provider(
        provider,
        model,
        catalog::SelectionAuthContext {
            current: Some(auth),
            available: &available,
        },
    )?;
    anyhow::ensure!(
        (selection.provider.as_str(), selection.model.as_str(), selection.auth.as_str())
            == (provider, model, auth),
        "agent '{agent_id}': saved model selection {provider}/{model} with auth '{auth}' is no longer supported"
    );
    anyhow::ensure!(
        credentials::auth_has_credentials(store, auth)?,
        "agent '{agent_id}': no credentials available for saved {provider}/{model} with auth '{auth}'"
    );
    Ok(Config {
        provider: selection.provider,
        model: selection.model,
        auth: selection.auth,
        model_alias: None,
        ..host.clone()
    })
}

#[cfg(test)]
#[path = "agent_binding_model_override_tests.rs"]
mod tests;
