use std::{collections::HashSet, time::Duration};

use serde::Deserialize;

use crate::model::{ModelError, ReasoningCapabilities};

use super::ProviderModel;

const MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";

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
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
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
                .map(|model| model.into_provider_model(provider)),
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
    models.sort_by(|left, right| left.model.cmp(&right.model));
    models.dedup_by(|left, right| left.model == right.model);
    Ok(models)
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
    fn supports_generate_content(&self) -> bool {
        self.supported_generation_methods
            .iter()
            .any(|method| method == "generateContent")
    }

    fn into_provider_model(self, provider: &str) -> ProviderModel {
        let id = self
            .name
            .strip_prefix("models/")
            .unwrap_or(&self.name)
            .to_string();
        let reasoning_capabilities = match self.thinking {
            Some(false) => ReasoningCapabilities::NotConfigurable,
            Some(true) | None => ReasoningCapabilities::Unknown,
        };
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
