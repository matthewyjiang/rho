use serde_json::{json, Value};

use crate::model::{ModelError, ModelIdentity, ModelRequest};
use rho_sdk::model::{ServiceTier, ToolSpec};

use crate::protocol::openai_responses::{
    codex_input_items_for_target, codex_reasoning_param, to_responses_tool, ToolStrictness,
};

use super::auth::Auth;
use super::reasoning::OpenAiReasoningProfile;

/// Complete wire policy for one OpenAI Responses endpoint variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponsesWireContract {
    OpenAiStandard,
    CodexStandard,
}

impl ResponsesWireContract {
    fn for_auth(auth: &Auth) -> Self {
        match auth {
            Auth::ApiKey(_) => Self::OpenAiStandard,
            Auth::Codex { .. } => Self::CodexStandard,
        }
    }

    fn provider(self) -> &'static str {
        match self {
            Self::OpenAiStandard => "openai",
            Self::CodexStandard => "openai-codex",
        }
    }

    pub(super) fn default_api_base(self) -> &'static str {
        match self {
            Self::OpenAiStandard => "https://api.openai.com/v1",
            Self::CodexStandard => "https://chatgpt.com/backend-api/codex",
        }
    }

    pub(super) fn uses_codex_websocket(self) -> bool {
        match self {
            Self::OpenAiStandard => false,
            Self::CodexStandard => true,
        }
    }

    fn parallel_tool_calls(self) -> Option<bool> {
        match self {
            Self::OpenAiStandard => None,
            Self::CodexStandard => Some(true),
        }
    }

    fn serialize_tool(self, tool: ToolSpec, hosted_web_search: bool) -> Value {
        let strictness = match self {
            Self::OpenAiStandard => ToolStrictness::Explicit(false),
            Self::CodexStandard => ToolStrictness::Unspecified,
        };
        to_responses_tool(tool, strictness, hosted_web_search)
    }
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
        let contract = ResponsesWireContract::for_auth(auth);
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

/// Shared lowered fields for Responses create and compact bodies.
struct ResponsesLowered {
    instructions: String,
    input: Vec<Value>,
    prompt_cache_key: Option<String>,
    reasoning: Option<Value>,
}

/// Lowers request history into common Responses fields.
fn lower_responses_request(
    profile: &ResponsesProfile,
    reasoning_profile: &OpenAiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<ResponsesLowered, ModelError> {
    let reasoning =
        reasoning_profile.config(profile.provider(), profile.model(), request.reasoning_level)?;
    let mut instructions = Vec::new();
    let input = codex_input_items_for_target(
        request.messages.to_vec(),
        &mut instructions,
        Some(profile.identity()),
    )?;
    let reasoning =
        codex_reasoning_param(reasoning.effort.as_deref(), reasoning.summary.as_deref());
    Ok(ResponsesLowered {
        instructions: instructions.join("\n\n"),
        input,
        prompt_cache_key: request.prompt_cache_key.map(str::to_owned),
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
    prompt_cache_key: Option<String>,
    reasoning: Option<Value>,
) {
    if let Some(prompt_cache_key) = prompt_cache_key {
        body["prompt_cache_key"] = json!(prompt_cache_key);
    }
    if let Some(reasoning) = reasoning {
        body["reasoning"] = reasoning;
    }
}

/// Builds a streaming Responses create body for one model turn.
pub(super) fn build_responses_create_body(
    profile: &ResponsesProfile,
    reasoning_profile: &OpenAiReasoningProfile,
    request: ModelRequest<'_>,
    service_tier: Option<ServiceTier>,
    hosted_web_search: bool,
) -> Result<Value, ModelError> {
    let contract = profile.contract();
    let tools = request
        .tools
        .iter()
        .cloned()
        .map(|tool| contract.serialize_tool(tool, hosted_web_search))
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
    body["instructions"] = json!(instructions);
    body["input"] = json!(input);
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
    }
    if let Some(parallel_tool_calls) = contract.parallel_tool_calls() {
        body["parallel_tool_calls"] = json!(parallel_tool_calls);
    }
    attach_prompt_cache_and_reasoning(&mut body, prompt_cache_key, reasoning);
    body["include"] = json!(["reasoning.encrypted_content"]);
    Ok(body)
}

/// Builds a unary `/responses/compact` body.
///
/// Compact never advertises tools and never streams.
pub(super) fn build_responses_compact_body(
    profile: &ResponsesProfile,
    reasoning_profile: &OpenAiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    let ResponsesLowered {
        instructions,
        input,
        prompt_cache_key,
        reasoning,
    } = lower_responses_request(profile, reasoning_profile, request)?;
    let mut body = base_responses_body(profile);
    body["instructions"] = json!(instructions);
    body["input"] = json!(input);
    attach_prompt_cache_and_reasoning(&mut body, prompt_cache_key, reasoning);
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
pub(super) fn build_codex_responses_body(
    model: &str,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    build_codex_responses_body_with_tier(model, request, None, /*hosted_web_search*/ true)
}

#[cfg(test)]
fn build_codex_responses_body_with_tier(
    model: &str,
    request: ModelRequest<'_>,
    service_tier: Option<ServiceTier>,
    hosted_web_search: bool,
) -> Result<Value, ModelError> {
    let profile = ResponsesProfile::from_auth(&codex_test_auth(), model);
    build_responses_create_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        request,
        service_tier,
        hosted_web_search,
    )
}

#[cfg(test)]
#[path = "codex_request_tests.rs"]
mod tests;
