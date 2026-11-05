//! The login record of a host logged in through its endpoint (Ollama).
//!
//! `[providers.<name>]` records that login, with or without a key. Every
//! login path records it and every logout path forgets it, so this module is
//! the one place the TUI, `rho login`, and ACP change it.

use rho_providers::provider::ProviderDescriptor;

use super::config_repository::ConfigRepository;

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
