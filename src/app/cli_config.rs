use crate::{
    cli::Cli,
    config::Config,
    credentials::{self, CredentialStore},
    model::{catalog, provider_models::refresh_provider_models_with_store},
    provider::{self, ProviderModelSource},
};

pub(super) fn validate(cli: &Cli) -> anyhow::Result<()> {
    if cli.resume.is_some() && cli.command.is_some() {
        anyhow::bail!("--resume is only supported for interactive sessions");
    }
    Ok(())
}

pub(super) async fn refresh_model_cache(
    cli: &Cli,
    store: &dyn CredentialStore,
) -> anyhow::Result<()> {
    let provider = cli
        .provider
        .as_deref()
        .or_else(|| cli.model.as_deref().and_then(explicit_model_provider));
    if let Some(provider) = provider {
        refresh_model_list_for_provider(provider, store).await?;
    }
    Ok(())
}

pub(super) fn apply_overrides(config: &mut Config, cli: &Cli) -> anyhow::Result<bool> {
    let mut save_config = false;
    if let Some(provider) = &cli.provider {
        apply_provider_override(config, provider, cli.model.is_some())?;
        save_config = true;
    }
    if let Some(model) = &cli.model {
        apply_model_override(config, model)?;
        save_config = true;
    }
    if let Some(auth) = &cli.auth {
        config.auth = auth.clone();
        save_config = true;
    }
    if let Some(reasoning) = cli.reasoning {
        config.reasoning = reasoning;
        save_config = true;
    }
    let supported_reasoning =
        crate::model::models_dev::cached_reasoning_levels(&config.provider, &config.model);
    let reasoning = config.reasoning.normalize(supported_reasoning.as_deref());
    if reasoning != config.reasoning {
        config.reasoning = reasoning;
        save_config = true;
    }
    Ok(save_config)
}

fn apply_provider_override(
    config: &mut Config,
    provider: &str,
    has_model_override: bool,
) -> anyhow::Result<()> {
    if !catalog::implemented_providers().contains(&provider) {
        anyhow::bail!("unknown provider '{provider}' for --provider");
    }
    let auth = catalog::login_target_for_provider(provider).map(|target| target.auth);
    let model = if has_model_override {
        None
    } else {
        Some(catalog::default_model_for_provider(provider).ok_or_else(|| {
            anyhow::anyhow!(
                "no cached models for provider '{provider}'. Run /refresh-model-list {provider} or pass a cached provider/model with --model"
            )
        })?)
    };
    config.provider = provider.to_string();
    if let Some(auth) = auth {
        config.auth = auth;
    }
    if let Some(model) = model {
        config.model = model;
    }
    Ok(())
}

fn apply_model_override(config: &mut Config, model: &str) -> anyhow::Result<()> {
    let selection = catalog::resolve_model_selection_for_auths(
        model,
        &config.provider,
        &config.auth,
        std::slice::from_ref(&config.auth),
    )?;
    config.provider = selection.provider;
    config.model = selection.model;
    config.auth = selection.auth;
    Ok(())
}

async fn refresh_model_list_for_provider(
    provider: &str,
    store: &dyn credentials::CredentialStore,
) -> anyhow::Result<()> {
    let Some(descriptor) = provider::provider_descriptor(provider) else {
        return Ok(());
    };
    if descriptor.model_refresh.is_none() || catalog::default_model_for_provider(provider).is_some()
    {
        return Ok(());
    }
    match refresh_provider_models_with_store(provider, store).await {
        Ok(_) => Ok(()),
        Err(error) if provider_requires_cached_models(provider) => Err(error.into()),
        Err(_) => Ok(()),
    }
}

fn provider_requires_cached_models(provider: &str) -> bool {
    provider::provider_descriptor(provider)
        .map(|descriptor| descriptor.model_source == ProviderModelSource::CachedProviderModels)
        .unwrap_or(false)
}

fn explicit_model_provider(model: &str) -> Option<&str> {
    let (provider, model) = model.trim().split_once('/')?;
    (!provider.trim().is_empty() && !model.trim().is_empty()).then_some(provider.trim())
}

#[cfg(test)]
#[path = "cli_config_tests.rs"]
mod tests;
