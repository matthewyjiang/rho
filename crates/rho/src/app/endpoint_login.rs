//! The login record of a host logged in through its endpoint (Ollama).
//!
//! `[providers.<name>]` records that login, with or without a key. Every
//! login path records it and every logout path forgets it, so this module is
//! the one place the TUI, `rho login`, and ACP change it.

use std::{borrow::Cow, path::Path};

use rho_providers::provider::ProviderDescriptor;

use super::config_repository::ConfigRepository;
use crate::config::Config;

/// `config` with the login record of its provider taken from the config
/// file at `config_path`, for building that provider. Refuses, as a missing
/// credential, when the file records no login.
///
/// A long-lived process (ACP, a TUI session) holds config loaded at startup,
/// but a logout or login since then (an ACP `logout`, another process)
/// changes the file. Reading it here makes that count the way a deleted or
/// saved key does, and builds against the endpoint the login chose. Other
/// providers, and every other setting, keep `config` as given.
pub(crate) fn with_disk_login<'a>(
    config: &'a Config,
    config_path: &Path,
) -> anyhow::Result<Cow<'a, Config>> {
    let needs_login = rho_providers::provider::provider_descriptor(&config.provider)
        .is_some_and(|descriptor| descriptor.collects_login_endpoint());
    if !needs_login {
        return Ok(Cow::Borrowed(config));
    }
    let on_disk = ConfigRepository::new(Some(config_path.to_path_buf())).load()?;
    let mut config = config.clone();
    config.providers.ollama = on_disk.providers.ollama;
    config.providers.require_endpoint_login(&config.provider)?;
    Ok(Cow::Owned(config))
}

/// Records a finished login to a host logged in through its endpoint, at
/// its default API base when none is set. `rho login` and ACP sign-in collect
/// no URL, but the login must count the same as the TUI's, key or no key.
/// Leaves config untouched for every other provider.
pub(crate) fn record(
    repository: &ConfigRepository,
    descriptor: ProviderDescriptor,
) -> anyhow::Result<()> {
    if !repository
        .load()?
        .providers
        .awaits_endpoint_login(descriptor)
    {
        return Ok(());
    }
    repository
        .update(|config| config.providers.record_login(descriptor))?
        .map(drop)
}

/// Forgets a login to a host logged in through its endpoint and drops its
/// cached models, so pickers stop listing it at once. Returns whether a
/// login was recorded; other providers are left untouched. Dropping the
/// cache is best-effort once the login is gone: a leftover list is purged
/// at the next start, so it only logs.
pub(crate) fn forget(
    repository: &ConfigRepository,
    descriptor: ProviderDescriptor,
) -> anyhow::Result<bool> {
    if !descriptor.collects_login_endpoint()
        || repository
            .load()?
            .providers
            .awaits_endpoint_login(descriptor)
    {
        return Ok(false);
    }
    repository.update(|config| config.providers.forget_login(descriptor))?;
    if let Err(error) =
        rho_providers::model::provider_models::forget_provider_models(descriptor.name)
    {
        tracing::warn!(provider = descriptor.name, %error, "could not drop cached models");
    }
    Ok(true)
}
