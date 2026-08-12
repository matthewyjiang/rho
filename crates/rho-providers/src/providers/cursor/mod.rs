//! Cursor AgentService provider (OAuth + Connect/protobuf Run).
//!
//! Login stays on `api2.cursor.sh`. AgentService (`Run`, `GetUsableModels`)
//! lives on `agentn.global.api5.cursor.sh` and only accepts HTTP/2. The shared
//! reqwest client negotiates h2 through TLS ALPN (`native-tls-alpn`); HTTP/1.1
//! is rejected by the load balancer with 464.

use crate::{
    auth::cursor_token::CursorAuthManager,
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse},
};

mod models;
mod stream;

pub(crate) use models::fetch_usable_models;

/// Returns whether `/fast` can request Cursor's trailing `-fast` model variant.
pub fn supports_fast_mode(model: &str) -> bool {
    crate::protocol::cursor::supports_fast_mode(model)
}

const CURSOR_CLIENT_VERSION: &str = "cli-2026.01.09-231024f";
const RUN_PATH: &str = "/agent.v1.AgentService/Run";
const MODELS_PATH: &str = "/agent.v1.AgentService/GetUsableModels";

pub struct CursorProvider {
    client: reqwest::Client,
    auth: CursorAuthManager,
    model: String,
    api_base: String,
}

impl CursorProvider {
    pub(crate) fn new_with_transport(
        model: String,
        auth: CursorAuthManager,
        client: reqwest::Client,
        api_base: String,
    ) -> Self {
        Self {
            client,
            auth,
            model,
            api_base,
        }
    }

    pub(crate) fn model_identity(&self) -> ModelIdentity {
        ModelIdentity::new("cursor", "cursor-agent", &self.model)
    }

    fn api_base(&self) -> &str {
        self.api_base.trim_end_matches('/')
    }

    fn run_url(&self) -> String {
        format!("{}{RUN_PATH}", self.api_base())
    }

    fn apply_headers(
        builder: reqwest::RequestBuilder,
        access_token: &str,
        streaming: bool,
    ) -> reqwest::RequestBuilder {
        let mut builder = builder
            .version(reqwest::Version::HTTP_2)
            .bearer_auth(access_token)
            .header("User-Agent", crate::rho_user_agent())
            .header("x-ghost-mode", "true")
            .header("x-cursor-client-version", CURSOR_CLIENT_VERSION)
            .header("x-cursor-client-type", "cli")
            .header("x-request-id", request_id());
        if streaming {
            builder = builder
                .header("Content-Type", "application/connect+proto")
                .header("connect-protocol-version", "1")
                .header("te", "trailers");
        } else {
            builder = builder.header("Content-Type", "application/proto");
        }
        builder
    }

    pub(crate) async fn complete_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        self.stream_turn(request, &mut |_| Ok(()), &mut |_| Ok(()))
            .await
    }

    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        self.stream_turn_with_options(
            request,
            rho_sdk::provider::ModelRequestOptions::default(),
            on_event,
            on_request_event,
        )
        .await
    }

    pub(crate) async fn stream_turn_with_options(
        &self,
        request: ModelRequest<'_>,
        options: rho_sdk::provider::ModelRequestOptions,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        stream::run_turn(self, request, options, on_event, on_request_event).await
    }
}

crate::impl_sdk_model_provider!(CursorProvider, request_options);

/// AWS ALBs return 464 when the client speaks HTTP/1.1 to an HTTP/2 target.
fn incompatible_protocol(status: reqwest::StatusCode) -> Option<ModelError> {
    (status.as_u16() == 464).then(|| {
        ModelError::InvalidResponse(
            "Cursor AgentService requires HTTP/2; the load balancer rejected HTTP/1.1 (HTTP 464)"
                .into(),
        )
    })
}

fn request_id() -> String {
    let mut bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod stream_tests;
