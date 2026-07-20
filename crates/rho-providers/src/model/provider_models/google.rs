use std::{collections::HashSet, time::Duration};

use futures_util::stream::{self, StreamExt};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

use crate::model::ModelError;

use super::ProviderModel;

#[path = "google_policy.rs"]
mod policy;

pub(crate) use policy::{
    is_text_chat_model, reasoning_capabilities, thinking_policy, ThinkingPolicy,
};

const MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const LIST_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const PROBE_CONCURRENCY: usize = 8;

pub(super) async fn fetch(
    provider: &str,
    api_key: String,
) -> Result<Vec<ProviderModel>, ModelError> {
    fetch_from(provider, api_key, MODELS_URL).await
}

async fn fetch_from(
    provider: &str,
    api_key: String,
    endpoint: &str,
) -> Result<Vec<ProviderModel>, ModelError> {
    let client = reqwest::Client::builder().timeout(LIST_TIMEOUT).build()?;
    let mut models = Vec::new();
    let mut page_token = None::<String>;
    let mut seen_page_tokens = HashSet::new();
    loop {
        let mut request = client.get(endpoint).header("x-goog-api-key", &api_key);
        if let Some(token) = &page_token {
            request = request.query(&[("pageToken", token)]);
        }
        let response: ModelsResponse = request.send().await?.error_for_status()?.json().await?;
        models.extend(
            response
                .models
                .into_iter()
                .filter(Model::supports_generate_content)
                .filter(|model| is_text_chat_model(model.id())),
        );
        let Some(token) = response.next_page_token.filter(|token| !token.is_empty()) else {
            break;
        };
        if !seen_page_tokens.insert(token.clone()) {
            return Err(ModelError::InvalidResponse(
                "Google Models API repeated a page token".into(),
            ));
        }
        page_token = Some(token);
    }

    // Probe each id once. Duplicates across pages must not steal concurrency slots.
    let mut seen_ids = HashSet::new();
    models.retain(|model| seen_ids.insert(model.id().to_string()));

    let available = retain_available_models(&api_key, endpoint, models).await?;
    let mut models = available
        .into_iter()
        .map(|model| model.into_provider_model(provider))
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.model.cmp(&right.model));
    Ok(models)
}

async fn retain_available_models(
    api_key: &str,
    models_endpoint: &str,
    models: Vec<Model>,
) -> Result<Vec<Model>, ModelError> {
    if models.is_empty() {
        return Ok(models);
    }
    let probe_client = reqwest::Client::builder().timeout(PROBE_TIMEOUT).build()?;
    let base = generate_content_base(models_endpoint)?;
    let mut available = Vec::with_capacity(models.len());
    let mut stream = stream::iter(models.into_iter().map(|model| {
        let client = probe_client.clone();
        let api_key = api_key.to_string();
        let base = base.clone();
        async move {
            let availability = probe_model_availability(&client, &api_key, &base, model.id()).await;
            (model, availability)
        }
    }))
    .buffer_unordered(PROBE_CONCURRENCY);

    while let Some((model, availability)) = stream.next().await {
        match availability? {
            ModelAvailability::Available | ModelAvailability::Transient => available.push(model),
            ModelAvailability::Unavailable => {}
        }
    }
    // Keep list order stable after concurrent probes.
    available.sort_by(|left, right| left.id().cmp(right.id()));
    Ok(available)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelAvailability {
    Available,
    Transient,
    Unavailable,
}

async fn probe_model_availability(
    client: &reqwest::Client,
    api_key: &str,
    api_base: &str,
    model_id: &str,
) -> Result<ModelAvailability, ModelError> {
    let url = format!(
        "{}/models/{}:generateContent",
        api_base.trim_end_matches('/'),
        model_id
    );
    let response = client
        .post(url)
        .header("x-goog-api-key", api_key)
        .json(&json!({
            "contents": [{
                "role": "user",
                "parts": [{"text": "."}]
            }],
            "generationConfig": {
                "maxOutputTokens": 1
            }
        }))
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        // Transport failures are not proof the model is retired.
        Err(_) => return Ok(ModelAvailability::Transient),
    };
    let status = response.status();
    if status.is_success() {
        return Ok(ModelAvailability::Available);
    }
    let body = response.text().await.unwrap_or_default();
    Ok(classify_probe_status(status, &body))
}

fn classify_probe_status(status: StatusCode, body: &str) -> ModelAvailability {
    if status == StatusCode::NOT_FOUND || body_indicates_retired_model(body) {
        return ModelAvailability::Unavailable;
    }
    if body_indicates_permanent_key_or_region_block(body) {
        return ModelAvailability::Unavailable;
    }
    if matches!(
        status.as_u16(),
        401 | 403 | 408 | 429 | 500 | 502 | 503 | 504
    ) {
        return ModelAvailability::Transient;
    }
    // Unknown permanent client errors still keep the model visible so a probe
    // quirk does not hide a usable model from the catalog.
    ModelAvailability::Transient
}

fn body_indicates_retired_model(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("no longer available")
}

fn body_indicates_permanent_key_or_region_block(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("failed_precondition")
        || lower.contains("not available in your country")
        || lower.contains("user location is not supported")
        || lower.contains("consumer_suspended")
}

fn generate_content_base(models_endpoint: &str) -> Result<String, ModelError> {
    let trimmed = models_endpoint.trim_end_matches('/');
    Ok(trimmed
        .strip_suffix("/models")
        .unwrap_or(trimmed)
        .to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelsResponse {
    #[serde(default)]
    models: Vec<Model>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    name: String,
    display_name: Option<String>,
    input_token_limit: Option<u64>,
    output_token_limit: Option<u64>,
    thinking: Option<bool>,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
}

impl Model {
    fn id(&self) -> &str {
        self.name.strip_prefix("models/").unwrap_or(&self.name)
    }

    fn supports_generate_content(&self) -> bool {
        self.supported_generation_methods
            .iter()
            .any(|method| method == "generateContent")
    }

    fn into_provider_model(self, provider: &str) -> ProviderModel {
        let id = self.id().to_string();
        let reasoning_capabilities = reasoning_capabilities(&id, self.thinking);
        ProviderModel {
            provider: provider.to_string(),
            display_name: self.display_name.unwrap_or_else(|| id.clone()),
            model: id,
            context_window: self.input_token_limit,
            max_output_tokens: self.output_token_limit,
            reasoning_capabilities,
        }
    }
}

#[cfg(test)]
#[path = "google_tests.rs"]
mod tests;
