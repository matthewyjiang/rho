use crate::{
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse},
    protocol::gemini_generate_content::{
        build_request, collect_stream, GenerateContentResponse, ResponseCollector, ThinkingConfig,
        ThinkingLevel,
    },
};

pub const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GoogleProvider {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
    model: String,
}

impl GoogleProvider {
    pub(crate) fn new_with_transport(
        model: String,
        api_key: String,
        client: reqwest::Client,
        api_base: String,
    ) -> Self {
        Self {
            client,
            api_key,
            api_base,
            model: model.strip_prefix("models/").unwrap_or(&model).to_string(),
        }
    }

    fn request_body(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<crate::protocol::gemini_generate_content::GenerateContentRequest, ModelError> {
        build_request(
            request.messages,
            request.tools,
            &self.model_identity(),
            thinking_config(&self.model, request.reasoning_level)?,
        )
    }

    pub(crate) fn model_identity(&self) -> ModelIdentity {
        ModelIdentity::new("google", "gemini-generate-content", &self.model)
    }

    pub(crate) async fn complete_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let body = self.request_body(request)?;
        let response = self
            .client
            .post(self.url("generateContent")?)
            .header("x-goog-api-key", &self.api_key)
            .json(&body)
            .send()
            .await?;
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
        let response: GenerateContentResponse = response.json().await?;
        let mut collector = ResponseCollector::default();
        collector.apply(response, None)?;
        collector.finish()
    }

    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        _on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = self.send_stream(request, on_event) => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        }
    }

    async fn send_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let body = self.request_body(request)?;
        let response = self
            .client
            .post(self.url("streamGenerateContent")?)
            .query(&[("alt", "sse")])
            .header("x-goog-api-key", &self.api_key)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&body)
            .send()
            .await?;
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
        collect_stream(response, on_event).await
    }

    fn url(&self, method: &str) -> Result<url::Url, ModelError> {
        let mut url = url::Url::parse(self.api_base.trim_end_matches('/')).map_err(|error| {
            ModelError::InvalidResponse(format!("invalid Google API endpoint: {error}"))
        })?;
        url.path_segments_mut()
            .map_err(|_| {
                ModelError::InvalidResponse("Google API endpoint cannot be a base URL".into())
            })?
            .push("models")
            .push(&format!("{}:{method}", self.model));
        Ok(url)
    }
}

fn thinking_config(
    model: &str,
    level: crate::reasoning::ReasoningLevel,
) -> Result<Option<ThinkingConfig>, ModelError> {
    use crate::reasoning::ReasoningLevel;

    let include_thoughts = level != ReasoningLevel::Off;
    if model.starts_with("gemini-3") {
        if level == ReasoningLevel::Off
            || (model.contains("pro") && level == ReasoningLevel::Minimal)
            || (model.contains("flash-lite-image")
                && matches!(level, ReasoningLevel::Low | ReasoningLevel::Medium))
        {
            return Err(ModelError::UnsupportedReasoning {
                provider: "google",
                model: model.to_string(),
                requested: level,
            });
        }
        let thinking_level = match level {
            ReasoningLevel::Off | ReasoningLevel::Minimal => ThinkingLevel::Minimal,
            ReasoningLevel::Low => ThinkingLevel::Low,
            ReasoningLevel::Medium => ThinkingLevel::Medium,
            ReasoningLevel::High | ReasoningLevel::Xhigh | ReasoningLevel::Max => {
                ThinkingLevel::High
            }
        };
        return Ok(Some(ThinkingConfig {
            thinking_budget: None,
            thinking_level: Some(thinking_level),
            include_thoughts,
        }));
    }
    if !model.starts_with("gemini-2.5") {
        if level == ReasoningLevel::Off {
            return Ok(None);
        }
        return Err(ModelError::UnsupportedReasoning {
            provider: "google",
            model: model.to_string(),
            requested: level,
        });
    }
    if model.contains("pro") && level == ReasoningLevel::Off {
        return Err(ModelError::UnsupportedReasoning {
            provider: "google",
            model: model.to_string(),
            requested: level,
        });
    }
    let mut budget = match level {
        ReasoningLevel::Off => 0,
        ReasoningLevel::Minimal => 1_024,
        ReasoningLevel::Low => 2_048,
        ReasoningLevel::Medium => 8_192,
        ReasoningLevel::High => 16_384,
        ReasoningLevel::Xhigh => 24_576,
        ReasoningLevel::Max => 32_768,
    };
    if model.contains("flash") {
        budget = budget.min(24_576);
    }
    Ok(Some(ThinkingConfig {
        thinking_budget: Some(budget),
        thinking_level: None,
        include_thoughts,
    }))
}

crate::impl_sdk_model_provider!(GoogleProvider);

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
