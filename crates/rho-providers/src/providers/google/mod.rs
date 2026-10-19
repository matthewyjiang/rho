use crate::{
    model::{
        models_dev::known_reasoning_capabilities,
        provider_models::{thinking_policy, ThinkingPolicy},
        ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ReasoningCapabilities,
    },
    protocol::gemini_generate_content::{
        build_request, collect_stream, GenerateContentResponse, ResponseCollector, ThinkingConfig,
        ThinkingLevel,
    },
    reasoning::ReasoningLevel,
};

pub const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GoogleProvider {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
    identity_provider: &'static str,
    model: String,
    thinking: GeminiThinking,
    session_headers: super::opencode::SessionHeaders,
}

impl GoogleProvider {
    /// First-party Gemini API.
    pub(crate) fn new_with_transport(
        model: String,
        api_key: String,
        client: reqwest::Client,
        api_base: String,
    ) -> Self {
        Self::new_with_identity(model, api_key, client, api_base, "google")
    }

    /// Hosted Gemini `generateContent` adapter registered under `identity_provider`.
    pub(crate) fn new_with_identity(
        model: String,
        api_key: String,
        client: reqwest::Client,
        api_base: String,
        identity_provider: &'static str,
    ) -> Self {
        let model = model.strip_prefix("models/").unwrap_or(&model).to_string();
        Self {
            client,
            api_key,
            api_base,
            identity_provider,
            thinking: GeminiThinking::resolve(
                thinking_policy(&model),
                known_reasoning_capabilities(identity_provider, &model),
            ),
            model,
            session_headers: super::opencode::SessionHeaders::new(identity_provider),
        }
    }

    fn request_body<'a>(
        &self,
        request: ModelRequest<'a>,
    ) -> Result<crate::protocol::gemini_generate_content::GenerateContentRequest<'a>, ModelError>
    {
        build_request(
            request.messages,
            request.tools,
            &self.model_identity(),
            self.thinking
                .config(request.reasoning_level)
                .map_err(|UnsupportedLevel| ModelError::UnsupportedReasoning {
                    provider: self.identity_provider,
                    model: self.model.clone(),
                    requested: request.reasoning_level,
                })?,
        )
    }

    pub(crate) fn model_identity(&self) -> ModelIdentity {
        ModelIdentity::new(
            self.identity_provider,
            "gemini-generate-content",
            &self.model,
        )
    }

    pub(crate) async fn complete_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let headers = self.session_headers.headers(request.prompt_cache_key);
        let body = self.request_body(request)?;
        let response = super::request_body::send(
            self.client
                .post(self.url("generateContent")?)
                .headers(headers)
                .header("x-goog-api-key", &self.api_key)
                .json(&body),
            super::request_body::EndpointCompression::Verified,
        )
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
        self.send_stream(request, on_event).await
    }

    async fn send_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let headers = self.session_headers.headers(request.prompt_cache_key);
        let body = self.request_body(request)?;
        let response = super::request_body::send(
            self.client
                .post(self.url("streamGenerateContent")?)
                .query(&[("alt", "sse")])
                .headers(headers)
                .header("x-goog-api-key", &self.api_key)
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .json(&body),
            super::request_body::EndpointCompression::Verified,
        )
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

/// The model does not accept the requested reasoning level.
#[derive(Debug, PartialEq, Eq)]
struct UnsupportedLevel;

/// Thinking controls for one Gemini model, resolved when the provider is built.
#[derive(Clone, Debug, PartialEq, Eq)]
enum GeminiThinking {
    /// Gemini 2.5: a token budget per level, from the model-id table.
    Budget {
        levels: &'static [ReasoningLevel],
        flash_cap: bool,
    },
    /// A named `thinkingLevel` per request. These models always think, so
    /// `Off` is rejected rather than silently running at the default level.
    Levels(Vec<ReasoningLevel>),
    /// No thinking controls: only `Off`, sent without a `thinkingConfig`.
    Fixed,
}

impl GeminiThinking {
    /// Gemini 2.5 keeps its budget table. Everything else uses the known
    /// levels for the provider, the same ones pickers offer (models.dev for
    /// gateways; first-party Google's discovered list wins once refreshed),
    /// else the model-id table, which predates ids like `gemini-4-*`.
    fn resolve(policy: ThinkingPolicy, catalog: ReasoningCapabilities) -> Self {
        match (policy, catalog) {
            (ThinkingPolicy::Budget { levels, flash_cap }, _) => Self::Budget { levels, flash_cap },
            (_, ReasoningCapabilities::Levels(levels)) => Self::Levels(levels.into_levels()),
            (ThinkingPolicy::Level { levels }, _) => Self::Levels(levels.to_vec()),
            (ThinkingPolicy::None, _) => Self::Fixed,
        }
    }

    /// The request's `thinkingConfig`; `Ok(None)` sends none.
    fn config(&self, level: ReasoningLevel) -> Result<Option<ThinkingConfig>, UnsupportedLevel> {
        let include_thoughts = level != ReasoningLevel::Off;
        match self {
            Self::Budget { levels, flash_cap } => {
                if !levels.contains(&level) {
                    return Err(UnsupportedLevel);
                }
                let budget = match level {
                    ReasoningLevel::Off => 0,
                    ReasoningLevel::Minimal => 1_024,
                    ReasoningLevel::Low => 2_048,
                    ReasoningLevel::Medium => 8_192,
                    ReasoningLevel::High => 16_384,
                    ReasoningLevel::Xhigh => 24_576,
                    ReasoningLevel::Max => 32_768,
                };
                Ok(Some(ThinkingConfig {
                    thinking_budget: Some(if *flash_cap {
                        budget.min(24_576)
                    } else {
                        budget
                    }),
                    thinking_level: None,
                    include_thoughts,
                }))
            }
            Self::Levels(levels) => {
                let thinking_level = match level {
                    _ if !levels.contains(&level) => return Err(UnsupportedLevel),
                    ReasoningLevel::Off => return Err(UnsupportedLevel),
                    ReasoningLevel::Minimal => ThinkingLevel::Minimal,
                    ReasoningLevel::Low => ThinkingLevel::Low,
                    ReasoningLevel::Medium => ThinkingLevel::Medium,
                    ReasoningLevel::High | ReasoningLevel::Xhigh | ReasoningLevel::Max => {
                        ThinkingLevel::High
                    }
                };
                Ok(Some(ThinkingConfig {
                    thinking_budget: None,
                    thinking_level: Some(thinking_level),
                    include_thoughts,
                }))
            }
            Self::Fixed if level == ReasoningLevel::Off => Ok(None),
            Self::Fixed => Err(UnsupportedLevel),
        }
    }
}

crate::impl_sdk_model_provider!(GoogleProvider);

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;
