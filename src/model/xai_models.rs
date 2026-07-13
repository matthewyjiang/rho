use reqwest::StatusCode;
use serde::Deserialize;

use crate::{
    auth::xai_token::{
        auth_material_with_store, force_refresh_auth_material_with_store, XaiAuthSource,
    },
    credentials::CredentialStore,
    model::{provider_models::ProviderModel, ModelError},
};

const LANGUAGE_MODELS_URL: &str = "https://api.x.ai/v1/language-models";

#[derive(Deserialize)]
struct LanguageModelsResponse {
    models: Vec<LanguageModel>,
}

#[derive(Deserialize)]
struct LanguageModel {
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    max_output_tokens: Option<u64>,
}

pub(super) async fn fetch_models(
    provider: &str,
    store: &dyn CredentialStore,
) -> Result<Vec<ProviderModel>, ModelError> {
    let client = reqwest::Client::new();
    let material = auth_material_with_store(&client, store).await?;
    let response = send_request(&client, &material.access_token).await?;
    let response = if response.status() == StatusCode::UNAUTHORIZED
        && material.source == XaiAuthSource::Store
    {
        if let Some(refreshed) =
            force_refresh_auth_material_with_store(&client, store, &material.access_token).await?
        {
            send_request(&client, &refreshed.access_token).await?
        } else {
            response
        }
    } else {
        response
    };
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return if status == StatusCode::UNAUTHORIZED {
            Err(ModelError::MissingXaiAuth)
        } else {
            Err(ModelError::HttpStatus { status, body })
        };
    }
    parse_models(provider, response.json::<LanguageModelsResponse>().await?)
}

async fn send_request(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<reqwest::Response, ModelError> {
    Ok(client
        .get(LANGUAGE_MODELS_URL)
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .header("User-Agent", concat!("rho/", env!("CARGO_PKG_VERSION")))
        .send()
        .await?)
}

fn parse_models(
    provider: &str,
    response: LanguageModelsResponse,
) -> Result<Vec<ProviderModel>, ModelError> {
    let mut models = response
        .models
        .into_iter()
        .flat_map(|model| {
            let max_output_tokens = model.max_output_tokens;
            std::iter::once(model.id)
                .chain(model.aliases)
                .map(move |id| ProviderModel {
                    provider: provider.to_string(),
                    display_name: id.clone(),
                    model: id,
                    max_output_tokens,
                })
        })
        .filter(|model| !model.model.trim().is_empty())
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.model.cmp(&right.model));
    models.dedup_by(|left, right| left.model == right.model);
    if models.is_empty() {
        return Err(ModelError::InvalidResponse(
            "xAI language models response was empty".into(),
        ));
    }
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ids_and_aliases_and_deduplicates() {
        let models = parse_models(
            "xai",
            LanguageModelsResponse {
                models: vec![
                    LanguageModel {
                        id: "grok-4.5-20260520".into(),
                        aliases: vec!["grok-4.5".into()],
                        max_output_tokens: Some(64_000),
                    },
                    LanguageModel {
                        id: "grok-4.5".into(),
                        aliases: Vec::new(),
                        max_output_tokens: None,
                    },
                ],
            },
        )
        .unwrap();

        assert_eq!(
            models,
            vec![
                ProviderModel {
                    provider: "xai".into(),
                    model: "grok-4.5".into(),
                    display_name: "grok-4.5".into(),
                    max_output_tokens: Some(64_000),
                },
                ProviderModel {
                    provider: "xai".into(),
                    model: "grok-4.5-20260520".into(),
                    display_name: "grok-4.5-20260520".into(),
                    max_output_tokens: Some(64_000),
                },
            ]
        );
    }
}
