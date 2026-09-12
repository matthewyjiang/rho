//! Offline model selection for editing global model instructions.

use std::io::IsTerminal;

use anyhow::{ensure, Context};
use rho_providers::{
    model::{catalog, favorites},
    provider,
};

use super::{picker::standalone, PickerItem, PickerKeyHints, PickerLayout, UiPicker};
use crate::config::Config;

pub(crate) async fn select(
    config: &Config,
    provider_filter: Option<&str>,
) -> anyhow::Result<Option<catalog::ModelCatalogEntry>> {
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "model selection requires an interactive terminal; pass --model <MODEL> to edit directly"
    );
    config.providers.activate()?;
    // Editing instructions needs no credentials. Include every registered auth
    // mode so the shared catalog also supplies cached and keyless providers.
    let auths = provider::providers()
        .iter()
        .flat_map(|descriptor| descriptor.auth_modes())
        .map(|mode| mode.id.to_owned())
        .collect::<Vec<_>>();
    let mut entries = catalog::available_models_for_auths(&auths);
    let current = config.model_aliases.resolve(&config.model)?;
    let current_provider = current.provider.as_deref().unwrap_or(&config.provider);
    let current_provider = provider::legacy_provider_alias(current_provider)
        .map_or(current_provider, |(canonical, _)| canonical);
    // A configured pass-through model may not have a catalog entry yet.
    if !entries
        .iter()
        .any(|entry| entry.provider == current_provider && entry.model == current.model)
    {
        entries.push(catalog::ModelCatalogEntry {
            provider: current_provider.to_owned(),
            model: current.model.clone(),
            display_name: current.model.clone(),
            auth_modes: Vec::new(),
        });
    }
    entries.retain(|entry| provider_filter.is_none_or(|filter| entry.provider == filter));
    ensure!(
        !entries.is_empty(),
        "no locally known models for provider '{}'; pass --model <MODEL> to edit directly",
        provider_filter.unwrap_or("all")
    );
    let favorites = favorites::normalized_favorite_models(&config.favorite_models);
    let entries = favorites::reorder_models_by_favorites(entries, &favorites);
    let items = entries
        .iter()
        .map(|entry| {
            let reference = provider::model_reference(&entry.provider, &entry.model);
            PickerItem {
                label: reference.clone(),
                section: None,
                detail: None,
                preview: None,
                badge: None,
                value: reference,
                selection_verb: Some("edit"),
                allow_filter_completion: true,
            }
        })
        .collect();
    let mut picker = UiPicker::models("Select model prompt", items)
        .with_layout(PickerLayout::Overlay)
        .with_confirm_verb("edit")
        .with_key_hints(PickerKeyHints {
            tab_complete: true,
            ..Default::default()
        });
    picker.selected = entries
        .iter()
        .position(|entry| entry.provider == current_provider && entry.model == current.model)
        .unwrap_or(0);
    let Some(selected) = standalone::select(picker, &config.keybindings, &config.theme).await?
    else {
        return Ok(None);
    };
    entries
        .into_iter()
        .find(|entry| provider::model_reference(&entry.provider, &entry.model) == selected)
        .map(Some)
        .context("selected model is no longer in the picker")
}
