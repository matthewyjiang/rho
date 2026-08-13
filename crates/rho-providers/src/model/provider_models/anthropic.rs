use reqwest::Url;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    credentials::CredentialStore,
    model::{ModelError, ReasoningCapabilities},
};

use super::{load_api_key_auth, provider_models_client, ProviderModel};

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

pub(super) async fn fetch(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<super::ProviderModelRecord>, ModelError> {
    let key = load_api_key_auth(provider, store)?;
    let client = provider_models_client()?;
    let mut models = Vec::new();
    let mut after_id = None::<String>;
    loop {
        let mut url = Url::parse("https://api.anthropic.com/v1/models").map_err(|err| {
            ModelError::InvalidResponse(format!("invalid Anthropic models URL: {err}"))
        })?;
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
        models.extend(
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
                }),
        );
        if !response.has_more {
            break;
        }
        let Some(next_after_id) = last_id else {
            break;
        };
        after_id = Some(next_after_id);
    }
    models.sort_by(|left, right| left.model.model.cmp(&right.model.model));
    models.dedup_by(|left, right| left.model.model == right.model.model);
    Ok(models)
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
