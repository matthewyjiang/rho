//! Native Ollama `/api/tags` + `/api/show` discovery.
//!
//! Ollama's OpenAI-compatible `/v1/models` only returns ids. The native
//! endpoints supply context length and thinking capability.

use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    credentials::CredentialStore,
    model::{ModelError, ReasoningCapabilities, ReasoningLevelSet},
    provider::{self, OLLAMA_UNKNOWN_REASONING_LEVELS},
};

use super::{provider_models_client, ProviderModel};

/// Upper bound on `/api/show` calls during one refresh so a host with many
/// incomplete tags rows cannot stall startup.
const MAX_SHOW_LOOKUPS: usize = 16;

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    #[serde(default)]
    models: Vec<OllamaTagModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaTagModel {
    name: String,
    #[serde(default)]
    details: OllamaModelDetails,
    capabilities: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct OllamaModelDetails {
    context_length: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct OllamaShowResponse {
    #[serde(default)]
    details: OllamaModelDetails,
    capabilities: Option<Vec<String>>,
    model_info: Option<Map<String, Value>>,
}

/// Strips a trailing `/v1` segment so `http://host:11434/v1` becomes the
/// native root `http://host:11434/`. Bases that are not `/v1`-suffixed skip
/// native discovery.
fn native_root(api_base: &Url) -> Option<Url> {
    let mut root = api_base.clone();
    root.set_query(None);
    root.set_fragment(None);
    let segments = root
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.last().copied() != Some("v1") {
        return None;
    }
    let path = if segments.len() == 1 {
        "/".to_string()
    } else {
        format!("/{}/", segments[..segments.len() - 1].join("/"))
    };
    root.set_path(&path);
    Some(root)
}

fn reasoning_capabilities_from(capabilities: Option<&[String]>) -> ReasoningCapabilities {
    let Some(capabilities) = capabilities else {
        return ReasoningCapabilities::Unknown;
    };
    if capabilities
        .iter()
        .any(|capability| capability == "thinking")
    {
        ReasoningCapabilities::Levels(ReasoningLevelSet::new(
            OLLAMA_UNKNOWN_REASONING_LEVELS.to_vec(),
        ))
    } else {
        ReasoningCapabilities::NotConfigurable
    }
}

/// Embedding-only models are not a coding-agent surface. Capability-less rows
/// stay; the server may be older than the capabilities field.
fn is_chat_model(capabilities: Option<&[String]>) -> bool {
    let Some(capabilities) = capabilities else {
        return true;
    };
    let embedding = capabilities
        .iter()
        .any(|capability| capability == "embedding");
    let completion = capabilities
        .iter()
        .any(|capability| capability == "completion");
    !embedding || completion
}

fn context_length_from_show(response: &OllamaShowResponse) -> Option<u64> {
    if let Some(info) = &response.model_info {
        for (key, value) in info {
            if key.ends_with(".context_length") {
                if let Some(window) = json_u64(value).filter(|window| *window > 0) {
                    return Some(window);
                }
            }
        }
    }
    response.details.context_length.filter(|window| *window > 0)
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
}

pub(super) async fn fetch(
    descriptor: &provider::ProviderDescriptor,
    auth: provider::AuthMode,
    api_base: &Url,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let Some(root) = native_root(api_base) else {
        return super::openai_compatible::fetch(descriptor, auth, api_base, store).await;
    };
    match fetch_tags(&root).await {
        Ok(models) => Ok(hydrate_models(descriptor, &root, models).await),
        Err(_) => super::openai_compatible::fetch(descriptor, auth, api_base, store).await,
    }
}

async fn fetch_tags(root: &Url) -> Result<Vec<OllamaTagModel>, ModelError> {
    let url = root.join("api/tags").map_err(|error| {
        ModelError::InvalidResponse(format!("invalid Ollama tags URL: {error}"))
    })?;
    let response = provider_models_client()?.get(url).send().await?;
    if !response.status().is_success() {
        return Err(ModelError::InvalidResponse(format!(
            "Ollama /api/tags returned {}",
            response.status()
        )));
    }
    let response: OllamaTagsResponse = response.json().await.map_err(|error| {
        ModelError::InvalidResponse(format!("invalid Ollama /api/tags response: {error}"))
    })?;
    Ok(response.models)
}

async fn hydrate_models(
    descriptor: &provider::ProviderDescriptor,
    root: &Url,
    models: Vec<OllamaTagModel>,
) -> Vec<ProviderModel> {
    let mut show_budget = MAX_SHOW_LOOKUPS;
    let lookups = models.into_iter().map(|model| {
        let name = model.name.trim().to_string();
        let skip = name.is_empty() || !is_chat_model(model.capabilities.as_deref());
        let needs_show = !skip
            && (model.details.context_length.filter(|window| *window > 0).is_none()
                || model.capabilities.is_none());
        let should_show = needs_show && show_budget > 0;
        if should_show {
            show_budget -= 1;
        }
        async move {
            if skip {
                return None;
            }
            let mut capabilities = model.capabilities;
            let mut context_window = model.details.context_length.filter(|window| *window > 0);
            if should_show {
                if let Some(shown) = fetch_show(root, &name).await {
                    context_window = context_length_from_show(&shown).or(context_window);
                    if shown.capabilities.is_some() {
                        capabilities = shown.capabilities;
                    }
                }
            }
            chat_provider_model(descriptor, &name, context_window, capabilities.as_deref())
        }
    });
    let mut complete = futures_util::future::join_all(lookups)
        .await
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    complete.sort_by(|left, right| left.model.cmp(&right.model));
    complete.dedup_by(|left, right| left.model == right.model);
    complete
}

fn chat_provider_model(
    descriptor: &provider::ProviderDescriptor,
    name: &str,
    context_window: Option<u64>,
    capabilities: Option<&[String]>,
) -> Option<ProviderModel> {
    let name = name.trim();
    if name.is_empty() || !is_chat_model(capabilities) {
        return None;
    }
    Some(ProviderModel {
        provider: descriptor.name.into(),
        display_name: name.to_string(),
        context_window,
        max_output_tokens: None,
        model: name.to_string(),
        reasoning_capabilities: reasoning_capabilities_from(capabilities),
    })
}

async fn fetch_show(root: &Url, model: &str) -> Option<OllamaShowResponse> {
    let url = root.join("api/show").ok()?;
    let response = provider_models_client()
        .ok()?
        .post(url)
        .json(&json!({ "model": model }))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

#[cfg(test)]
#[path = "ollama_tests.rs"]
mod tests;
