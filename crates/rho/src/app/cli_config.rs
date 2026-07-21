use {
    crate::cli::Cli,
    crate::config::Config,
    crate::model_aliases::ResolvedModelReference,
    rho_providers::credentials::{self, CredentialStore},
    rho_providers::model::{
        catalog,
        models_dev::{cached_reasoning_capabilities, current_reasoning_capabilities},
        provider_models::{
            provider_model_capabilities_need_refresh, refresh_provider_models_with_store,
            ProviderModelEndpoint,
        },
    },
    rho_providers::provider::{self, ProviderModelSource},
};

pub(super) enum ProviderRefreshStatus {
    NotAttempted,
    Attempted { provider: String },
}

impl ProviderRefreshStatus {
    fn was_attempted_for(&self, provider: &str) -> bool {
        matches!(self, Self::Attempted { provider: attempted } if attempted == provider)
    }
}

pub(super) fn validate(cli: &Cli) -> anyhow::Result<()> {
    if cli.resume.is_some() && cli.command.is_some() {
        anyhow::bail!("--resume is only supported for interactive sessions");
    }
    Ok(())
}

pub(super) async fn refresh_model_cache(
    cli: &Cli,
    config: &Config,
    store: &dyn CredentialStore,
) -> anyhow::Result<ProviderRefreshStatus> {
    let model_override = cli
        .model
        .as_deref()
        .map(|reference| effective_model_override(config, reference, cli.provider.as_deref()))
        .transpose()?;
    let provider = model_override
        .as_ref()
        .and_then(|selection| selection.provider.as_deref())
        .or(cli.provider.as_deref())
        .or_else(|| {
            model_override
                .as_ref()
                .and_then(|selection| explicit_model_provider(&selection.model))
        })
        .unwrap_or(&config.provider);
    if cli.provider.is_none()
        && cli.model.is_none()
        && (provider != "kimi-code"
            || !provider_model_capabilities_need_refresh(provider, &config.model))
    {
        return Ok(ProviderRefreshStatus::NotAttempted);
    }
    let selected_model = model_override
        .as_ref()
        .map(|selection| {
            selection
                .model
                .trim()
                .strip_prefix(&format!("{provider}/"))
                .unwrap_or(selection.model.trim())
                .to_string()
        })
        .or_else(|| selected_model_for_refresh(config, provider));
    let endpoint = config.resolved_provider_endpoint(provider);
    let model_endpoint = endpoint.as_ref().map_or(
        ProviderModelEndpoint::ProviderOwned,
        ProviderModelEndpoint::OpenAiCompatible,
    );
    let attempted = refresh_model_list_for_provider(
        provider,
        selected_model.as_deref(),
        /*explicit_selection*/ cli.provider.is_some() || cli.model.is_some(),
        store,
        model_endpoint,
    )
    .await?;
    Ok(if attempted {
        ProviderRefreshStatus::Attempted {
            provider: provider.to_string(),
        }
    } else {
        ProviderRefreshStatus::NotAttempted
    })
}

pub(super) fn apply_overrides(config: &mut Config, cli: &Cli) -> anyhow::Result<bool> {
    let mut save_config = false;
    if let Some(provider) = &cli.provider {
        apply_provider_override(config, provider, cli.model.is_some())?;
        save_config = true;
    }
    if let Some(model) = &cli.model {
        apply_model_override(config, model, cli.provider.as_deref())?;
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
    Ok(save_config)
}

pub(super) fn normalize_reasoning_for_cli(
    config: &mut Config,
    source: rho_providers::model::ReasoningRequestSource,
) -> anyhow::Result<bool> {
    let capabilities = if source == rho_providers::model::ReasoningRequestSource::Explicit {
        current_reasoning_capabilities(&config.provider, &config.model)
    } else {
        cached_reasoning_capabilities(&config.provider, &config.model)
    };
    let resolution = capabilities.resolve(config.reasoning, source);
    if let rho_providers::model::ReasoningResolution::UnsupportedExplicit(requested) = resolution {
        let supported = capabilities
            .levels()
            .map(|levels| {
                levels
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "none".to_string());
        anyhow::bail!(
            "provider '{}' model '{}' does not support reasoning level '{}'; supported levels: {}",
            config.provider,
            config.model,
            requested,
            supported
        );
    }
    if source == rho_providers::model::ReasoningRequestSource::Explicit
        && resolution == rho_providers::model::ReasoningResolution::NotConfigurable
    {
        anyhow::bail!(
            "provider '{}' model '{}' does not expose configurable reasoning",
            config.provider,
            config.model
        );
    }
    Ok(apply_reasoning_resolution(config, resolution))
}

pub(super) async fn prepare_model_metadata(
    config: &Config,
    store: &dyn CredentialStore,
    provider_refresh: &ProviderRefreshStatus,
) {
    if needs_startup_capability_refresh(config, provider_refresh) {
        let endpoint = config.resolved_provider_endpoint(&config.provider);
        let model_endpoint = endpoint.as_ref().map_or(
            ProviderModelEndpoint::ProviderOwned,
            ProviderModelEndpoint::OpenAiCompatible,
        );
        let _ = refresh_provider_models_with_store(&config.provider, store, model_endpoint).await;
    }
    // models.dev metadata is optional and fetched asynchronously by the TUI.
    // Blocking automation and background-agent startup on the full catalog makes
    // cold or offline launches depend on an unrelated network request. Provider-
    // native discovery remains synchronous above because Kimi uses it as the
    // authoritative capability source.
}

fn needs_startup_capability_refresh(
    config: &Config,
    provider_refresh: &ProviderRefreshStatus,
) -> bool {
    config.provider == "kimi-code"
        && !provider_refresh.was_attempted_for(&config.provider)
        && provider_model_capabilities_need_refresh(&config.provider, &config.model)
}

pub(super) fn normalize_reasoning(config: &mut Config) -> bool {
    normalize_reasoning_from(
        config,
        rho_providers::model::ReasoningRequestSource::PersistedOrDefault,
    )
}

fn normalize_reasoning_from(
    config: &mut Config,
    source: rho_providers::model::ReasoningRequestSource,
) -> bool {
    let capabilities = cached_reasoning_capabilities(&config.provider, &config.model);
    let resolution = capabilities.resolve(config.reasoning, source);
    apply_reasoning_resolution(config, resolution)
}

fn apply_reasoning_resolution(
    config: &mut Config,
    resolution: rho_providers::model::ReasoningResolution,
) -> bool {
    let Some(reasoning) = resolution.effective() else {
        return false;
    };
    if reasoning == config.reasoning {
        return false;
    }
    config.reasoning = reasoning;
    true
}

pub(super) fn apply_provider_override(
    config: &mut Config,
    provider: &str,
    has_model_override: bool,
) -> anyhow::Result<()> {
    if !catalog::implemented_providers().contains(&provider) {
        anyhow::bail!("unknown provider '{provider}' for --provider");
    }
    let auth = provider::provider_descriptor(provider).map(|descriptor| descriptor.auth);
    let model = if has_model_override {
        None
    } else {
        Some(catalog::default_model_for_provider(provider).ok_or_else(|| {
            anyhow::anyhow!(
                "no cached models for provider '{provider}'. Open /config and choose Refresh model lists, or pass a cached provider/model with --model"
            )
        })?)
    };
    config.provider = provider.to_string();
    if let Some(auth) = auth {
        config.auth = auth.to_string();
    }
    if let Some(model) = model {
        config.model = model;
    }
    Ok(())
}

fn apply_model_override(
    config: &mut Config,
    reference: &str,
    cli_provider: Option<&str>,
) -> anyhow::Result<()> {
    let model_override = effective_model_override(config, reference, cli_provider)?;
    let selection = match model_override.provider.as_deref() {
        Some(provider) => {
            catalog::resolve_model_selection_for_provider(provider, &model_override.model)?
        }
        None => catalog::resolve_model_selection_for_auths(
            &model_override.model,
            &config.provider,
            &config.auth,
            std::slice::from_ref(&config.auth),
        )?,
    };
    config.provider = selection.provider;
    config.model = selection.model;
    config.auth = selection.auth;
    config.model_alias = model_override.alias;
    Ok(())
}

fn effective_model_override(
    config: &Config,
    reference: &str,
    cli_provider: Option<&str>,
) -> anyhow::Result<ResolvedModelReference> {
    // Resolve once before refresh or catalog validation so both paths act on
    // the same concrete target.
    let mut resolved = config
        .model_aliases
        .resolve(reference)
        .map_err(|error| anyhow::anyhow!("--model: {error}"))?;
    match (cli_provider, resolved.provider.as_deref(), &resolved.alias) {
        (Some(pinned), Some(alias_provider), Some(_)) if pinned != alias_provider => {
            anyhow::bail!(
                "model alias '{reference}' resolves to provider '{alias_provider}', which conflicts with --provider {pinned}"
            );
        }
        _ => {}
    }
    resolved.provider = resolved
        .provider
        .or_else(|| cli_provider.map(str::to_string));
    Ok(resolved)
}

async fn refresh_model_list_for_provider(
    provider: &str,
    selected_model: Option<&str>,
    explicit_selection: bool,
    store: &dyn credentials::CredentialStore,
    endpoint: ProviderModelEndpoint<'_>,
) -> anyhow::Result<bool> {
    let Some(descriptor) = provider::provider_descriptor(provider) else {
        return Ok(false);
    };
    let needs_model_discovery = catalog::default_model_for_provider(provider).is_none()
        && (selected_model.is_none() || provider_requires_cached_models(provider));
    let needs_capabilities =
        selected_model.is_some_and(|model| needs_synchronous_capability_refresh(provider, model));
    if descriptor.model_refresh.is_none() || (!needs_model_discovery && !needs_capabilities) {
        return Ok(false);
    }
    match refresh_provider_models_with_store(provider, store, endpoint).await {
        Ok(_) => Ok(true),
        Err(error)
            if explicit_selection
                && needs_model_discovery
                && provider_requires_cached_models(provider) =>
        {
            Err(error.into())
        }
        Err(_) => Ok(true),
    }
}

fn needs_synchronous_capability_refresh(provider: &str, model: &str) -> bool {
    provider == "kimi-code" && provider_model_capabilities_need_refresh(provider, model)
}

fn selected_model_for_refresh(config: &Config, provider: &str) -> Option<String> {
    if provider == config.provider {
        return Some(config.model.clone());
    }
    catalog::default_model_for_provider(provider)
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
