use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    credentials::CredentialStore,
    model::{ModelError, ReasoningCapabilities},
};

use super::{load_api_key_auth, provider_models_client, ProviderModel};

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
            let raw_json = model
                .capabilities
                .clone()
                .unwrap_or_else(|| Value::Object(Default::default()));
            super::ProviderModelRecord {
                model: ProviderModel {
                    provider: provider.to_string(),
                    display_name: model.display_name.unwrap_or_else(|| model.id.clone()),
                    context_window: model.max_input_tokens.filter(|window| *window > 0),
                    max_output_tokens: model.max_tokens,
                    model: model.id,
                    reasoning_capabilities: ReasoningCapabilities::Unknown,
                },
                raw_json,
            }
        })
        .collect()
}

/// Collapse fetched `/v1/models` pages into a complete snapshot.
///
/// Repeated or missing cursors end the list successfully. Stopping because the
/// page bound was hit while `has_more` still reports another page is an error so
/// a later cache replace cannot commit a partial catalog.
fn models_from_pages(
    provider: &str,
    pages: impl IntoIterator<Item = AnthropicModelsResponse>,
    max_pages: usize,
) -> Result<Vec<super::ProviderModelRecord>, ModelError> {
    let mut models = Vec::new();
    let mut after_id = None::<String>;
    let mut page_count = 0;
    let mut complete = false;
    for response in pages {
        page_count += 1;
        if page_count > max_pages {
            return Err(model_list_truncated(max_pages));
        }
        let has_more = response.has_more;
        let last_id = response.last_id.clone();
        models.extend(records_from_page(provider, response));
        match model_list_continuation(has_more, last_id, after_id.as_deref()) {
            ModelListContinuation::Done => {
                complete = true;
                break;
            }
            ModelListContinuation::Next {
                after_id: next_after_id,
            } => after_id = Some(next_after_id),
        }
    }
    if !complete {
        return Err(model_list_truncated(max_pages));
    }
    models.sort_by(|left, right| left.model.model.cmp(&right.model.model));
    models.dedup_by(|left, right| left.model.model == right.model.model);
    Ok(models)
}

pub(super) async fn fetch(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<super::ProviderModelRecord>, ModelError> {
    let key = load_api_key_auth(provider, store)?;
    let client = provider_models_client()?;
    let mut pages = Vec::new();
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
        let last_id = response.last_id.clone();
        let has_more = response.has_more;
        pages.push(response);
        match model_list_continuation(has_more, last_id, after_id.as_deref()) {
            ModelListContinuation::Done => break,
            ModelListContinuation::Next {
                after_id: next_after_id,
            } => after_id = Some(next_after_id),
        }
    }
    models_from_pages(provider, pages, MAX_MODEL_PAGES)
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
