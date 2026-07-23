use std::sync::{Arc, Mutex};

use serde_json::Value;

pub mod auth;
pub mod cache;
mod codex_continuation;
mod codex_request;
mod codex_ws;
mod reasoning;
mod remote_compaction;
mod responses_http;

pub use cache::prompt_cache_key_from_session_id;

use crate::protocol::openai_responses::collect_codex_sse_response;
use auth::Auth;
#[cfg(test)]
use codex_request::build_codex_responses_body;
use codex_request::{build_responses_create_body, ResponsesProfile};
use codex_ws::{CodexWsTransport, CodexWsTurn};
use reasoning::OpenAiReasoningProfile;
use responses_http::{ResponsesEndpoint, ResponsesHttpTransport};

use crate::{
    credentials::{CodexTokens, CredentialStore},
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage},
};

#[cfg(test)]
use crate::provider_backend::stream_timeout::provider_client;

pub struct OpenAiProvider {
    client: reqwest::Client,
    auth: Auth,
    api_base: String,
    profile: ResponsesProfile,
    reasoning: OpenAiReasoningProfile,
    codex_ws: CodexWsTransport,
    credential_store: Arc<dyn CredentialStore>,
    refreshed_codex_tokens: Mutex<Option<CodexTokens>>,
}

impl OpenAiProvider {
    #[cfg(test)]
    pub(crate) fn new_with_auth(
        model: String,
        auth: Auth,
        credential_store: Arc<dyn CredentialStore>,
    ) -> Self {
        Self::new_with_transport(model, auth, credential_store, provider_client(), None)
    }

    pub(crate) fn new_with_transport(
        model: String,
        auth: Auth,
        credential_store: Arc<dyn CredentialStore>,
        client: reqwest::Client,
        api_base_override: Option<String>,
    ) -> Self {
        let profile = ResponsesProfile::from_auth(&auth, model);
        let api_base = api_base_override.unwrap_or_else(|| profile.default_api_base().to_string());
        let reasoning = OpenAiReasoningProfile::from_metadata(
            crate::model::models_dev::current_model_metadata(profile.provider(), profile.model()),
        );
        let codex_ws = CodexWsTransport::new(&api_base);
        Self {
            client,
            auth,
            api_base,
            profile,
            reasoning,
            codex_ws,
            credential_store,
            refreshed_codex_tokens: Mutex::new(None),
        }
    }

    fn http(&self) -> ResponsesHttpTransport<'_> {
        ResponsesHttpTransport::new(
            &self.client,
            &self.api_base,
            &self.profile,
            self.credential_store.as_ref(),
            &self.refreshed_codex_tokens,
        )
    }

    fn create_body(&self, request: ModelRequest<'_>) -> Result<Value, ModelError> {
        build_responses_create_body(&self.profile, &self.reasoning, request)
    }

    #[cfg(test)]
    fn openai_api_responses_body(&self, request: ModelRequest<'_>) -> Result<Value, ModelError> {
        self.create_body(request)
    }
}

impl OpenAiProvider {
    pub(crate) fn model_identity(&self) -> ModelIdentity {
        self.profile.identity().clone()
    }

    /// Completes one turn using a `Send` future suitable for the public SDK trait.
    pub(crate) async fn complete_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        match &self.auth {
            Auth::ApiKey(_) => self.send_openai_api_responses_complete(request).await,
            Auth::Codex { .. } => self.send_codex_responses_complete(request).await,
        }
    }

    /// Streams one turn through a `Send` callback for the public SDK adapter.
    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = async {
                match &self.auth {
                    Auth::ApiKey(_) => {
                        self.send_openai_api_responses_stream(request, on_event).await
                    }
                    Auth::Codex { .. } => {
                        self.send_codex_responses_stream(request, on_event, on_request_event)
                            .await
                    }
                }
            } => result,
            () = cancellation.cancelled() => {
                if matches!(&self.auth, Auth::Codex { .. }) {
                    self.codex_ws.reset().await;
                }
                Err(ModelError::Interrupted)
            }
        }
    }
}

crate::impl_sdk_model_provider!(OpenAiProvider, native_compact);

impl OpenAiProvider {
    async fn send_codex_responses_complete(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let body = self.create_body(request)?;
        let tokens = self.http().codex_tokens_for_auth(&self.auth)?;
        match self
            .codex_ws
            .send_responses_turn_silent(body.clone(), &tokens, self.profile.mode())
            .await?
        {
            CodexWsTurn::Completed(response) => return Ok(response),
            CodexWsTurn::FullSseFallback { .. } => {}
        }

        let http_result = self
            .http()
            .post_json(
                &self.auth,
                ResponsesEndpoint::Create,
                &body,
                /*cancellation*/ None,
            )
            .await;
        let response = match http_result.response {
            Ok(response) => response,
            Err(err) => {
                self.codex_ws.reset().await;
                return Err(err);
            }
        };
        if !response.status().is_success() {
            self.codex_ws.reset().await;
            return Err(crate::provider_backend::http_error::from_response(response).await);
        }
        self.collect_codex_sse_response_silent(response, &body)
            .await
    }

    async fn send_codex_responses_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        self.send_codex_responses_inner(request, Some(on_event), on_request_event)
            .await
    }

    async fn send_codex_responses_inner(
        &self,
        request: ModelRequest<'_>,
        mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        let body = self.create_body(request.clone())?;
        let tokens = self.http().codex_tokens_for_auth(&self.auth)?;
        match self
            .codex_ws
            .send_responses_turn(body, &tokens, self.profile.mode(), &mut on_event)
            .await?
        {
            CodexWsTurn::Completed(response) => return Ok(response),
            CodexWsTurn::FullSseFallback { request_submitted } => {
                if request_submitted {
                    // The submitted WebSocket request may have reached the model
                    // before the transport failed, so account for it separately.
                    on_request_event(
                        rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                            kind: rho_sdk::ProviderErrorKind::Unavailable,
                            usage: ModelUsage::default(),
                        },
                    )?;
                }
            }
        }

        // Rebuilt only on this rare fallback path so the common WebSocket
        // turn does not clone the full-history request body.
        let body = self.create_body(request)?;
        let http_result = self
            .http()
            .post_json(
                &self.auth,
                ResponsesEndpoint::Create,
                &body,
                /*cancellation*/ None,
            )
            .await;
        for attempt in &http_result.failed_attempts {
            let kind = match attempt.kind {
                responses_http::ResponsesFailedAttemptKind::Authentication => {
                    rho_sdk::ProviderErrorKind::Authentication
                }
            };
            on_request_event(
                rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                    kind,
                    usage: ModelUsage::default(),
                },
            )?;
        }
        let response = match http_result.response {
            Ok(response) => response,
            Err(err) => {
                self.codex_ws.reset().await;
                return Err(err);
            }
        };
        if !response.status().is_success() {
            self.codex_ws.reset().await;
            return Err(crate::provider_backend::http_error::from_response(response).await);
        }
        self.collect_codex_sse_response(response, &mut on_event, &body)
            .await
    }

    async fn collect_codex_sse_response(
        &self,
        response: reqwest::Response,
        on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
        body: &Value,
    ) -> Result<ModelResponse, ModelError> {
        match collect_codex_sse_response(response, on_event).await {
            Ok(output) => {
                self.codex_ws
                    .record_full_request_success(body, &output)
                    .await?;
                Ok(output.response)
            }
            Err(err) => {
                self.codex_ws.reset().await;
                Err(err)
            }
        }
    }

    async fn collect_codex_sse_response_silent(
        &self,
        response: reqwest::Response,
        body: &Value,
    ) -> Result<ModelResponse, ModelError> {
        match crate::providers::send_stream::collect_codex_sse_silent(response).await {
            Ok(output) => {
                self.codex_ws
                    .record_full_request_success(body, &output)
                    .await?;
                Ok(output.response)
            }
            Err(err) => {
                self.codex_ws.reset().await;
                Err(err)
            }
        }
    }

    async fn native_compact_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<rho_sdk::provider::NativeCompactionResponse, ModelError> {
        Ok(remote_compaction::compact_with_http(
            &self.auth,
            &self.profile,
            &self.reasoning,
            &self.http(),
            &self.codex_ws,
            request,
        )
        .await)
    }

    async fn send_openai_api_responses_complete(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        let body = self.create_body(request)?;
        let http_result = self
            .http()
            .post_json(
                &self.auth,
                ResponsesEndpoint::Create,
                &body,
                Some(&cancellation),
            )
            .await;
        let response = http_result.response?;
        if !response.status().is_success() {
            return Err(crate::provider_backend::http_error::from_response(response).await);
        }
        crate::providers::send_stream::collect_codex_model_response_silent(response).await
    }

    async fn send_openai_api_responses_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        let body = self.create_body(request)?;
        let http_result = self
            .http()
            .post_json(
                &self.auth,
                ResponsesEndpoint::Create,
                &body,
                Some(&cancellation),
            )
            .await;
        let response = http_result.response?;
        if !response.status().is_success() {
            return Err(crate::provider_backend::http_error::from_response(response).await);
        }
        let mut on_event = Some(on_event);
        collect_codex_sse_response(response, &mut on_event)
            .await
            .map(|output| output.response)
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod stream_tests;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
