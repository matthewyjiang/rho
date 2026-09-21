//! xAI Responses provider (API key and OAuth).
//!
//! Create and compact are separate wire contracts:
//! - create peels system prompts into `instructions` and may attach tools/reasoning
//! - compact sends the full conversation in `input` (including system) and only
//!   accepts the documented `model` + `input` body fields

#[cfg(test)]
use std::sync::Arc;

use crate::protocol::openai_responses::collect_codex_sse_response;
use crate::providers::responses_http::{
    ResponsesEndpoint, ResponsesHttpAuth, ResponsesHttpResult, ResponsesHttpTransport,
};
use rho_sdk::provider::ModelRequestOptions;

use crate::{
    auth::xai_token::XaiAuthManager,
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse},
};

#[cfg(test)]
use crate::{credentials::CredentialStore, provider_backend::stream_timeout::provider_client};

mod bodies;
mod compact;
#[path = "reasoning.rs"]
mod reasoning;

#[cfg(test)]
#[path = "../xai_tests.rs"]
mod tests;

use bodies::build_xai_responses_body;
pub(crate) use bodies::XaiHostedTools;

#[cfg(test)]
use bodies::build_xai_compact_body;

pub struct XaiProvider {
    client: reqwest::Client,
    provider: &'static str,
    model: String,
    auth: XaiAuthManager,
    api_base: String,
    reasoning: reasoning::XaiReasoningProfile,
    hosted: XaiHostedTools,
}

impl XaiProvider {
    pub(crate) fn new_with_transport(
        provider: &'static str,
        model: String,
        auth: XaiAuthManager,
        client: reqwest::Client,
        api_base: String,
        hosted: XaiHostedTools,
    ) -> Self {
        let reasoning = reasoning::XaiReasoningProfile::from_metadata(
            &model,
            crate::model::models_dev::known_reasoning_metadata(provider, &model),
        );
        Self {
            client,
            provider,
            model,
            auth,
            api_base,
            reasoning,
            hosted,
        }
    }

    #[cfg(test)]
    fn new_with_api_base(
        model: String,
        store: Arc<dyn CredentialStore>,
        api_base: String,
    ) -> Result<Self, ModelError> {
        Ok(Self::new_with_transport(
            "xai",
            model,
            XaiAuthManager::new(store)?,
            provider_client(),
            api_base,
            XaiHostedTools::ALL,
        ))
    }

    /// Puts `grok-4.7-build-fast` on the body when this turn's service tier is
    /// priority. Replay identity stays [`Self::model_identity`].
    fn stamp_request_model(&self, body: &mut serde_json::Value, options: ModelRequestOptions) {
        let fast = options.service_tier() == Some(rho_sdk::model::ServiceTier::Priority);
        if fast
            && self.model == crate::providers::fast_mode::GROK_4_7
            && self.auth.allows_fast_request_model()
        {
            body["model"] = serde_json::Value::String(
                crate::providers::fast_mode::GROK_4_7_BUILD_FAST.to_string(),
            );
        }
    }

    fn stamped_create_body(
        &self,
        request: crate::model::ModelRequest<'_>,
        options: ModelRequestOptions,
    ) -> Result<serde_json::Value, ModelError> {
        let mut body = build_xai_responses_body(
            self.provider,
            &self.model,
            &self.reasoning,
            request,
            self.hosted,
        )?;
        self.stamp_request_model(&mut body, options);
        Ok(body)
    }

    pub(super) fn stamped_compact_body(
        &self,
        request: crate::model::ModelRequest<'_>,
        options: ModelRequestOptions,
    ) -> Result<serde_json::Value, ModelError> {
        let mut body = bodies::build_xai_compact_body(self.provider, &self.model, request)?;
        self.stamp_request_model(&mut body, options);
        Ok(body)
    }

    pub(super) fn http(&self) -> ResponsesHttpTransport<'_> {
        ResponsesHttpTransport::new(&self.client, &self.api_base)
    }

    fn responses_http_auth(access_token: &str) -> ResponsesHttpAuth {
        ResponsesHttpAuth::bearer(access_token, crate::rho_user_agent())
    }

    pub(super) async fn post_responses(
        &self,
        endpoint: ResponsesEndpoint,
        body: &serde_json::Value,
        cancellation: Option<&rho_sdk::CancellationToken>,
        before_retry: impl FnOnce() -> Result<(), ModelError>,
    ) -> ResponsesHttpResult {
        let material = match crate::provider_backend::cancel::cancel_aware(
            cancellation,
            self.auth.auth_material(),
        )
        .await
        {
            Ok(material) => material,
            Err(error) => return ResponsesHttpResult::err(error),
        };
        let request_auth = Self::responses_http_auth(&material.access_token);
        let access_token = material.access_token.clone();
        self.http()
            .post_json_refreshing(
                &request_auth,
                || async {
                    match self.auth.force_refresh(&access_token).await {
                        Ok(None) => Ok(None),
                        Ok(Some(refreshed)) => {
                            Ok(Some(Self::responses_http_auth(&refreshed.access_token)))
                        }
                        Err(error) => Err(error),
                    }
                },
                before_retry,
                endpoint,
                body,
                cancellation,
            )
            .await
    }

    async fn send_request(
        &self,
        request: ModelRequest<'_>,
        options: ModelRequestOptions,
        on_request_event: Option<
            &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                      + Send),
        >,
    ) -> Result<reqwest::Response, ModelError> {
        let cancellation = request.cancellation.clone();
        let body = self.stamped_create_body(request, options)?;
        let mut on_request_event = on_request_event;
        let http_result = self
            .post_responses(
                ResponsesEndpoint::Create,
                &body,
                Some(&cancellation),
                || {
                    if let Some(on_request_event) = on_request_event.as_mut() {
                        on_request_event(
                            rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                                kind: rho_sdk::ProviderErrorKind::Authentication,
                                usage: crate::model::ModelUsage::default(),
                            },
                        )?;
                    }
                    Ok(())
                },
            )
            .await;
        http_result.response
    }

    async fn send_responses_turn(
        &self,
        request: ModelRequest<'_>,
        options: ModelRequestOptions,
        mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
        on_request_event: Option<
            &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                      + Send),
        >,
    ) -> Result<ModelResponse, ModelError> {
        let response = self
            .send_request(request, options, on_request_event)
            .await?;
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
        collect_codex_sse_response(response, &mut on_event)
            .await
            .map(|output| output.response)
    }

    pub(crate) fn model_identity(&self) -> ModelIdentity {
        ModelIdentity::new(self.provider, "openai-responses", &self.model)
    }

    /// Completes one turn using a `Send` future suitable for the public SDK trait.
    pub(crate) async fn complete_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let response = self
            .send_request(request, ModelRequestOptions::default(), None)
            .await?;
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
        crate::providers::send_stream::collect_codex_model_response_silent(response).await
    }

    /// Streams one turn through a `Send` callback for the public SDK adapter.
    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        self.stream_turn_with_options(
            request,
            ModelRequestOptions::default(),
            on_event,
            on_request_event,
        )
        .await
    }

    pub(crate) async fn stream_turn_with_options(
        &self,
        request: ModelRequest<'_>,
        options: ModelRequestOptions,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        self.send_responses_turn(request, options, Some(on_event), Some(on_request_event))
            .await
    }
}

crate::impl_sdk_model_provider!(XaiProvider, native_compact_options, request_options);
