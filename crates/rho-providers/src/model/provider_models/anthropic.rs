//! Anthropic `/v1/models` discovery.
//!
//! Capability projection (thinking mode, effort, Off encoding) lives in
//! [`policy`] so fetch transport and wire/picker semantics stay separate.

use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;

use crate::{credentials::CredentialStore, model::ModelError};

use super::{load_api_key_auth, provider_models_client, ProviderModel};

#[path = "anthropic_policy.rs"]
mod policy;

pub(crate) use policy::{
    cached_thinking_mode, capabilities_json_is_known, dated_parent_model, AnthropicThinkingMode,
    OffThinking,
};
#[cfg(test)]
pub(crate) use policy::thinking_mode_from_value;

/// Upper bound on `/v1/models` pages so a misbehaving cursor cannot hang the
/// startup refresh.
const MAX_MODEL_PAGES: usize = 20;

#[derive(Deserialize)]
struct AnthropicModelsResponse {
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicModel {
    id: String,
    display_name: Option<String>,
    max_input_tokens: Option<u64>,
    max_tokens: Option<u64>,
    #[serde(default)]
    capabilities: Option<Value>,
}

enum ModelListContinuation {
    Done,
    Next { after_id: String },
}

fn model_list_continuation(
    has_more: bool,
    last_id: Option<String>,
    after_id: Option<&str>,
) -> ModelListContinuation {
    if !has_more {
        return ModelListContinuation::Done;
    }
    let Some(next_after_id) = last_id else {
        return ModelListContinuation::Done;
    };
    if after_id == Some(next_after_id.as_str()) {
        // The cursor did not advance; stop instead of refetching the page.
        return ModelListContinuation::Done;
    }
    ModelListContinuation::Next {
        after_id: next_after_id,
    }
}

fn model_list_truncated(max_pages: usize) -> ModelError {
    ModelError::InvalidResponse(format!(
        "Anthropic /v1/models exceeded {max_pages} pages while more results remain"
    ))
}

fn records_from_page(
    provider: &str,
    response: AnthropicModelsResponse,
) -> Vec<super::ProviderModelRecord> {
    response
        .data
        .into_iter()
        .filter(|model| model.id.starts_with("claude-"))
        .map(|model| {
            let raw_json = policy::capabilities_json(model.capabilities);
            // `capabilities_json` always yields a parseable object, including
            // `{}` when the API omitted capabilities.
            let reasoning_capabilities = policy::AnthropicModelCapabilities::from_value(&raw_json)
                .expect("capabilities_json always stores a parseable object")
                .reasoning_capabilities(&model.id);
            super::ProviderModelRecord {
                model: ProviderModel {
                    provider: provider.to_string(),
                    display_name: model.display_name.unwrap_or_else(|| model.id.clone()),
                    context_window: model.max_input_tokens.filter(|window| *window > 0),
                    max_output_tokens: model.max_tokens,
                    model: model.id,
                    reasoning_capabilities,
                },
                raw_json,
            }
        })
        .collect()
}

fn add_page(
    models: &mut Vec<super::ProviderModelRecord>,
    provider: &str,
    response: AnthropicModelsResponse,
    after_id: Option<&str>,
) -> ModelListContinuation {
    let has_more = response.has_more;
    let last_id = response.last_id.clone();
    models.extend(records_from_page(provider, response));
    model_list_continuation(has_more, last_id, after_id)
}

fn finalize_models(mut models: Vec<super::ProviderModelRecord>) -> Vec<super::ProviderModelRecord> {
    models.sort_by(|left, right| left.model.model.cmp(&right.model.model));
    models.dedup_by(|left, right| left.model.model == right.model.model);
    models
}

pub(super) async fn fetch(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<super::ProviderModelRecord>, ModelError> {
    let key = load_api_key_auth(provider, store)?;
    let client = provider_models_client()?;
    let mut models = Vec::new();
    let mut after_id = None::<String>;
    let base = Url::parse("https://api.anthropic.com/v1/models").map_err(|err| {
        ModelError::InvalidResponse(format!("invalid Anthropic models URL: {err}"))
    })?;
    for _ in 0..MAX_MODEL_PAGES {
        let mut url = base.clone();
        if let Some(after_id) = &after_id {
            url.query_pairs_mut().append_pair("after_id", after_id);
        }
        let response: AnthropicModelsResponse = client
            .get(url)
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        match add_page(&mut models, provider, response, after_id.as_deref()) {
            ModelListContinuation::Done => return Ok(finalize_models(models)),
            ModelListContinuation::Next {
                after_id: next_after_id,
            } => after_id = Some(next_after_id),
        }
    }
    Err(model_list_truncated(MAX_MODEL_PAGES))
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
