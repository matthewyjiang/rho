use serde_json::{json, Value};

use crate::model::{ModelError, ModelIdentity, ModelRequest};
use rho_sdk::model::{ServiceTier, ToolSpec};

use crate::protocol::openai_responses::{
    codex_input_items_for_target, codex_reasoning_param, to_responses_lite_tool, to_responses_tool,
    ToolStrictness,
};

use super::auth::Auth;
use super::reasoning::OpenAiReasoningProfile;
use super::responses_lite_image::prepare_responses_lite_messages;

/// Complete wire policy for one OpenAI Responses endpoint variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponsesWireContract {
    OpenAiStandard,
    CodexStandard,
    CodexLite,
}

impl ResponsesWireContract {
    fn for_auth(auth: &Auth, model: &str) -> Self {
        match auth {
            Auth::ApiKey(_) => Self::OpenAiStandard,
            Auth::Codex { .. } if is_responses_lite_model(model) => Self::CodexLite,
            Auth::Codex { .. } => Self::CodexStandard,
        }
    }

    fn provider(self) -> &'static str {
        match self {
            Self::OpenAiStandard => "openai",
            Self::CodexStandard | Self::CodexLite => "openai-codex",
        }
    }

    pub(super) fn default_api_base(self) -> &'static str {
        match self {
            Self::OpenAiStandard => "https://api.openai.com/v1",
            Self::CodexStandard | Self::CodexLite => "https://chatgpt.com/backend-api/codex",
        }
    }

    pub(super) fn uses_responses_lite(self) -> bool {
        match self {
            Self::OpenAiStandard | Self::CodexStandard => false,
            Self::CodexLite => true,
        }
    }

    pub(super) fn uses_codex_websocket(self) -> bool {
        match self {
            Self::OpenAiStandard => false,
            Self::CodexStandard | Self::CodexLite => true,
        }
    }

    /// Rho does not yet retain server output items in its continuation baseline.
    /// Lite tool turns therefore use full bodies so the prior function call is
    /// not duplicated in the next delta.
    pub(super) fn supports_incremental_websocket(self) -> bool {
        match self {
            Self::OpenAiStandard | Self::CodexLite => false,
            Self::CodexStandard => true,
        }
    }

    pub(super) fn uses_lite_transport_header(self) -> bool {
        match self {
            Self::OpenAiStandard | Self::CodexStandard => false,
            Self::CodexLite => true,
        }
    }

    fn include_encrypted_reasoning(self) -> bool {
        match self {
            Self::OpenAiStandard | Self::CodexStandard => true,
            Self::CodexLite => false,
        }
    }

    fn parallel_tool_calls(self) -> Option<bool> {
        match self {
            Self::OpenAiStandard => None,
            Self::CodexStandard => Some(true),
            Self::CodexLite => Some(false),
        }
    }

    fn serialize_tool(self, tool: ToolSpec) -> Value {
        match self {
            Self::OpenAiStandard => to_responses_tool(tool, ToolStrictness::Explicit(false)),
            Self::CodexStandard => to_responses_tool(tool, ToolStrictness::Unspecified),
            Self::CodexLite => to_responses_lite_tool(tool, ToolStrictness::Unspecified),
        }
    }
}

fn is_responses_lite_model(model: &str) -> bool {
    matches!(model, "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna")
}

/// Credential-derived Responses identity and wire contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResponsesProfile {
    model: String,
    identity: ModelIdentity,
    contract: ResponsesWireContract,
}

impl ResponsesProfile {
    pub(super) fn from_auth(auth: &Auth, model: impl Into<String>) -> Self {
        let model = model.into();
        let contract = ResponsesWireContract::for_auth(auth, &model);
        let provider = contract.provider();
        Self {
            identity: ModelIdentity::new(provider, "openai-responses", &model),
            model,
            contract,
        }
    }

    pub(super) fn provider(&self) -> &'static str {
        self.contract.provider()
    }

    pub(super) fn model(&self) -> &str {
        &self.model
    }

    pub(super) fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    pub(super) fn contract(&self) -> ResponsesWireContract {
        self.contract
    }

    pub(super) fn default_api_base(&self) -> &'static str {
        self.contract.default_api_base()
    }
}

/// A turn's history after contract-specific preparation, ready to lower.
struct ResponsesBodyRequest<'a> {
    messages: Vec<crate::model::Message>,
    tools: &'a [ToolSpec],
    reasoning_level: rho_sdk::ReasoningLevel,
    prompt_cache_key: Option<String>,
}

impl<'a> ResponsesBodyRequest<'a> {
    /// Applies whatever input preparation the wire contract requires.
    ///
    /// Responses Lite enforces its image limits here, so every body builder
    /// gets the same prepared history without repeating the policy check.
    async fn prepare(
        request: ModelRequest<'a>,
        contract: ResponsesWireContract,
    ) -> Result<Self, ModelError> {
        let ModelRequest {
            messages,
            tools,
            cancellation,
            reasoning_level,
            prompt_cache_key,
        } = request;
        let messages = messages.to_vec();
        let messages = if contract.uses_responses_lite() {
            prepare_responses_lite_messages(messages, &cancellation).await?
        } else {
            messages
        };
        Ok(Self {
            messages,
            tools,
            reasoning_level,
            prompt_cache_key: prompt_cache_key.map(str::to_owned),
        })
    }
}

/// Shared lowered fields for Responses create and compact bodies.
struct ResponsesLowered {
    instructions: String,
    input: Vec<Value>,
    prompt_cache_key: Option<String>,
    reasoning: Option<Value>,
}

/// Lowers an already prepared request into common Responses fields.
fn lower_responses_request(
    profile: &ResponsesProfile,
    reasoning_profile: &OpenAiReasoningProfile,
    request: ResponsesBodyRequest<'_>,
) -> Result<ResponsesLowered, ModelError> {
    let reasoning =
        reasoning_profile.config(profile.provider(), profile.model(), request.reasoning_level)?;
    let mut instructions = Vec::new();
    let input = codex_input_items_for_target(
        request.messages,
        &mut instructions,
        Some(profile.identity()),
    )?;
    let reasoning =
        codex_reasoning_param(reasoning.effort.as_deref(), reasoning.summary.as_deref());
    Ok(ResponsesLowered {
        instructions: instructions.join("\n\n"),
        input,
        prompt_cache_key: request.prompt_cache_key,
        reasoning,
    })
}

fn base_responses_body(profile: &ResponsesProfile) -> Value {
    json!({
        "model": profile.model(),
        "store": false,
    })
}

fn attach_prompt_cache_and_reasoning(
    body: &mut Value,
    contract: ResponsesWireContract,
    prompt_cache_key: Option<String>,
    reasoning: Option<Value>,
) {
    if let Some(prompt_cache_key) = prompt_cache_key {
        body["prompt_cache_key"] = json!(prompt_cache_key);
    }
    if let Some(mut reasoning) = reasoning {
        if contract == ResponsesWireContract::CodexLite {
            reasoning["context"] = json!("all_turns");
        }
        body["reasoning"] = reasoning;
    }
}

/// Builds a streaming Responses create body for one model turn.
pub(super) async fn build_responses_create_body(
    profile: &ResponsesProfile,
    reasoning_profile: &OpenAiReasoningProfile,
    request: ModelRequest<'_>,
    service_tier: Option<ServiceTier>,
) -> Result<Value, ModelError> {
    let contract = profile.contract();
    let request = ResponsesBodyRequest::prepare(request, contract).await?;
    let tools = request
        .tools
        .iter()
        .cloned()
        .map(|tool| contract.serialize_tool(tool))
        .collect::<Vec<_>>();
    let ResponsesLowered {
        instructions,
        input,
        prompt_cache_key,
        reasoning,
    } = lower_responses_request(profile, reasoning_profile, request)?;

    let mut body = base_responses_body(profile);
    body["stream"] = json!(true);
    if service_tier == Some(ServiceTier::Priority)
        && super::supports_fast_mode(profile.provider(), profile.model())
    {
        body["service_tier"] = json!("priority");
    }

    match contract {
        ResponsesWireContract::OpenAiStandard | ResponsesWireContract::CodexStandard => {
            body["instructions"] = json!(instructions);
            body["input"] = json!(input);
            if !tools.is_empty() {
                body["tools"] = json!(tools);
                body["tool_choice"] = json!("auto");
            }
        }
        ResponsesWireContract::CodexLite => {
            let mut lite_input = input;
            lite_input.insert(
                0,
                json!({
                    "type": "additional_tools",
                    "role": "developer",
                    "tools": tools,
                }),
            );
            if !instructions.is_empty() {
                lite_input.insert(
                    1,
                    json!({
                        "type": "message",
                        "role": "developer",
                        "content": [{
                            "type": "input_text",
                            "text": instructions,
                        }],
                    }),
                );
            }
            body["input"] = json!(lite_input);
            body["tool_choice"] = json!("auto");
        }
    }

    if let Some(parallel_tool_calls) = contract.parallel_tool_calls() {
        body["parallel_tool_calls"] = json!(parallel_tool_calls);
    }
    attach_prompt_cache_and_reasoning(&mut body, contract, prompt_cache_key, reasoning);
    if contract.include_encrypted_reasoning() {
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
    Ok(body)
}

/// Builds a unary `/responses/compact` body.
///
/// Compact never advertises tools and never streams.
pub(super) async fn build_responses_compact_body(
    profile: &ResponsesProfile,
    reasoning_profile: &OpenAiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    let contract = profile.contract();
    let request = ResponsesBodyRequest::prepare(request, contract).await?;
    let ResponsesLowered {
        instructions,
        input,
        prompt_cache_key,
        reasoning,
    } = lower_responses_request(profile, reasoning_profile, request)?;
    let mut body = base_responses_body(profile);

    match contract {
        ResponsesWireContract::OpenAiStandard | ResponsesWireContract::CodexStandard => {
            body["instructions"] = json!(instructions);
            body["input"] = json!(input);
        }
        ResponsesWireContract::CodexLite => {
            let mut lite_input = input;
            if !instructions.is_empty() {
                lite_input.insert(
                    0,
                    json!({
                        "type": "message",
                        "role": "developer",
                        "content": [{
                            "type": "input_text",
                            "text": instructions,
                        }],
                    }),
                );
            }
            body["input"] = json!(lite_input);
        }
    }

    attach_prompt_cache_and_reasoning(&mut body, contract, prompt_cache_key, reasoning);
    Ok(body)
}

#[cfg(test)]
pub(super) fn codex_test_auth() -> Auth {
    Auth::Codex {
        tokens: crate::credentials::CodexTokens {
            access_token: "test".into(),
            refresh_token: None,
            id_token: None,
            account_id: None,
        },
        source: super::auth::CodexAuthSource::Env,
    }
}

#[cfg(test)]
pub(super) async fn build_codex_responses_body(
    model: &str,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    build_codex_responses_body_with_tier(model, request, None).await
}

#[cfg(test)]
async fn build_codex_responses_body_with_tier(
    model: &str,
    request: ModelRequest<'_>,
    service_tier: Option<ServiceTier>,
) -> Result<Value, ModelError> {
    let profile = ResponsesProfile::from_auth(&codex_test_auth(), model);
    build_responses_create_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        request,
        service_tier,
    )
    .await
}

#[cfg(test)]
#[path = "codex_request_image_tests.rs"]
mod image_tests;

#[cfg(test)]
#[path = "codex_request_tests.rs"]
mod tests;
