use std::{collections::HashSet, time::Duration};

use serde::Deserialize;

use crate::model::{models_dev, ModelError};

use super::ProviderModel;

#[path = "google_policy.rs"]
mod policy;

pub(crate) use policy::{
    is_text_chat_model, reasoning_capabilities, thinking_policy, ThinkingPolicy,
};

const MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const LIST_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) async fn fetch(
    provider: &str,
    api_key: String,
) -> Result<Vec<ProviderModel>, ModelError> {
    let (models, deprecated_models) = tokio::join!(
        fetch_from(provider, api_key, MODELS_URL),
        models_dev::fetch_deprecated_provider_models(provider),
    );
    let mut models = models?;
    if let Some(deprecated_models) = &deprecated_models {
        hide_deprecated_models(&mut models, deprecated_models);
    }
    Ok(models)
}

fn hide_deprecated_models(models: &mut Vec<ProviderModel>, deprecated_models: &HashSet<String>) {
    models.retain(|model| !deprecated_models.contains(&model.model));
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

    let mut seen_ids = HashSet::new();
    models.retain(|model| seen_ids.insert(model.id().to_string()));

    let mut models = models
        .into_iter()
        .map(|model| model.into_provider_model(provider))
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.model.cmp(&right.model));
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
