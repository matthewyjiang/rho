use serde_json::{json, Value};

use crate::model::{ModelError, ModelIdentity, ModelRequest};
use rho_sdk::model::ToolSpec;

use crate::protocol::openai_responses::{
    codex_input_items_for_target, codex_reasoning_param, to_responses_lite_tool, to_responses_tool,
};

use super::auth::Auth;
use super::reasoning::OpenAiReasoningProfile;

/// Wire shape for OpenAI Responses create/compact bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponsesRequestMode {
    Standard,
    ResponsesLite,
}

impl ResponsesRequestMode {
    pub(super) fn for_model(model: &str) -> Self {
        match model {
            "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna" => Self::ResponsesLite,
            _ => Self::Standard,
        }
    }

    pub(super) fn uses_responses_lite(self) -> bool {
        matches!(self, Self::ResponsesLite)
    }

    /// Rho does not yet retain server output items in its continuation baseline.
    /// Responses Lite tool turns therefore use full request bodies so the
    /// model's previous function call is not duplicated in the next delta.
    pub(super) fn supports_incremental_websocket(self) -> bool {
        matches!(self, Self::Standard)
    }
}

/// Credential-derived Responses identity and request defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResponsesProfile {
    provider: &'static str,
    model: String,
    identity: ModelIdentity,
    mode: ResponsesRequestMode,
    flavor: ResponsesFlavor,
}

/// Auth flavor that owns endpoint headers and token refresh policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponsesFlavor {
    ApiKey,
    Codex,
}

impl ResponsesProfile {
    pub(super) fn from_auth(auth: &Auth, model: impl Into<String>) -> Self {
        let model = model.into();
        let (provider, flavor) = match auth {
            Auth::ApiKey(_) => ("openai", ResponsesFlavor::ApiKey),
            Auth::Codex { .. } => ("openai-codex", ResponsesFlavor::Codex),
        };
        let mode = match flavor {
            ResponsesFlavor::Codex => ResponsesRequestMode::for_model(&model),
            ResponsesFlavor::ApiKey => ResponsesRequestMode::Standard,
        };
        Self {
            provider,
            identity: ModelIdentity::new(provider, "openai-responses", &model),
            model,
            mode,
            flavor,
        }
    }

    pub(super) fn provider(&self) -> &'static str {
        self.provider
    }

    pub(super) fn model(&self) -> &str {
        &self.model
    }

    pub(super) fn identity(&self) -> &ModelIdentity {
        &self.identity
    }

    pub(super) fn mode(&self) -> ResponsesRequestMode {
        self.mode
    }

    pub(super) fn flavor(&self) -> ResponsesFlavor {
        self.flavor
    }

    pub(super) fn default_api_base(&self) -> &'static str {
        match self.flavor {
            ResponsesFlavor::Codex => "https://chatgpt.com/backend-api/codex",
            ResponsesFlavor::ApiKey => "https://api.openai.com/v1",
        }
    }
}

/// Shared lowered fields for Responses create and compact bodies.
struct ResponsesLowered {
    instructions: String,
    input: Vec<Value>,
    prompt_cache_key: Option<String>,
    reasoning: Option<Value>,
}

/// Lowers request history into instructions/input/reasoning/prompt_cache_key.
///
/// Tool conversion stays on the create path only.
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
    profile: &ResponsesProfile,
    prompt_cache_key: Option<String>,
    reasoning: Option<Value>,
) {
    if let Some(prompt_cache_key) = prompt_cache_key {
        body["prompt_cache_key"] = json!(prompt_cache_key);
    }
    if let Some(mut reasoning) = reasoning {
        if profile.mode() == ResponsesRequestMode::ResponsesLite {
            reasoning["context"] = json!("all_turns");
        }
        body["reasoning"] = reasoning;
    }
}

/// Builds a streaming Responses create body for a model turn.
pub(super) fn build_responses_create_body(
    profile: &ResponsesProfile,
    reasoning_profile: &OpenAiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    let tools = request
        .tools
        .iter()
        .map(|tool| responses_tool(profile.mode(), tool.clone()))
        .collect::<Vec<_>>();
    let ResponsesLowered {
        instructions,
        input,
        prompt_cache_key,
        reasoning,
    } = lower_responses_request(profile, reasoning_profile, request)?;

    let mut body = base_responses_body(profile);
    body["stream"] = json!(true);

    match profile.mode() {
        ResponsesRequestMode::Standard => {
            body["instructions"] = json!(instructions);
            body["input"] = json!(input);
            if !tools.is_empty() {
                body["tools"] = json!(tools);
                body["tool_choice"] = json!("auto");
            }
        }
        ResponsesRequestMode::ResponsesLite => {
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
            body["parallel_tool_calls"] = json!(false);
        }
    }

    attach_prompt_cache_and_reasoning(&mut body, profile, prompt_cache_key, reasoning);
    if profile.flavor() == ResponsesFlavor::ApiKey {
        body["include"] = json!(["reasoning.encrypted_content"]);
    }
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

    match profile.mode() {
        ResponsesRequestMode::Standard => {
            body["instructions"] = json!(instructions);
            body["input"] = json!(input);
        }
        ResponsesRequestMode::ResponsesLite => {
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

    attach_prompt_cache_and_reasoning(&mut body, profile, prompt_cache_key, reasoning);
    Ok(body)
}

fn responses_tool(mode: ResponsesRequestMode, tool: ToolSpec) -> Value {
    match mode {
        ResponsesRequestMode::Standard => to_responses_tool(tool),
        ResponsesRequestMode::ResponsesLite => to_responses_lite_tool(tool),
    }
}

#[cfg(test)]
pub(super) fn build_codex_responses_body(
    model: &str,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    let profile = ResponsesProfile::from_auth(
        &Auth::Codex {
            tokens: crate::credentials::CodexTokens {
                access_token: "test".into(),
                refresh_token: None,
                id_token: None,
                account_id: None,
            },
            source: super::auth::CodexAuthSource::Env,
        },
        model,
    );
    build_responses_create_body(&profile, &OpenAiReasoningProfile::unknown(), request)
}

#[cfg(test)]
mod tests {
    use super::super::auth::CodexAuthSource;
    use super::*;
    use crate::model::Message;

    #[test]
    fn gpt_5_6_models_use_responses_lite_without_incremental_websocket_continuation() {
        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let mode = ResponsesRequestMode::for_model(model);
            let body = build_codex_responses_body(
                model,
                ModelRequest {
                    messages: &[Message::user_text("hello")],
                    tools: &[ToolSpec {
                        name: "read_file".into(),
                        description: "read a file".into(),
                        input_schema: json!({"type": "object"}),
                    }],
                    cancellation: Default::default(),
                    reasoning_level: Default::default(),
                    prompt_cache_key: None,
                },
            )
            .unwrap();

            assert_eq!(mode, ResponsesRequestMode::ResponsesLite, "{model}");
            assert!(mode.uses_responses_lite(), "{model}");
            assert!(!mode.supports_incremental_websocket(), "{model}");
            assert_eq!(body["input"][0]["type"], "additional_tools", "{model}");
            assert_eq!(
                body["reasoning"],
                json!({"effort": "medium", "summary": "auto", "context": "all_turns"}),
                "{model}"
            );
        }
    }

    #[test]
    fn responses_lite_sets_all_turns_reasoning_context() {
        let body = build_codex_responses_body(
            "gpt-5.6-terra",
            ModelRequest {
                messages: &[Message::user_text("hello")],
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
        )
        .unwrap();

        assert_eq!(
            body["reasoning"],
            json!({"effort": "medium", "summary": "auto", "context": "all_turns"})
        );
    }

    #[test]
    fn responses_lite_moves_tools_and_instructions_into_input() {
        let body = build_codex_responses_body(
            "gpt-5.6-luna",
            ModelRequest {
                messages: &[
                    Message::System("follow the repository instructions".into()),
                    Message::user_text("fix the bug"),
                ],
                tools: &[ToolSpec {
                    name: "web_search".into(),
                    description: "search the web".into(),
                    input_schema: json!({"type": "object"}),
                }],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
        )
        .unwrap();

        assert!(body.get("instructions").is_none());
        assert!(body.get("tools").is_none());
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(
            body["input"][0],
            json!({
                "type": "additional_tools",
                "role": "developer",
                "tools": [{
                    "type": "function",
                    "name": "web_search",
                    "description": "search the web",
                    "parameters": {"type": "object"},
                    "strict": false,
                }],
            })
        );
        assert_eq!(
            body["input"][1],
            json!({
                "type": "message",
                "role": "developer",
                "content": [{
                    "type": "input_text",
                    "text": "follow the repository instructions",
                }],
            })
        );
    }

    #[test]
    fn standard_requests_keep_hosted_web_search_tool() {
        let body = build_codex_responses_body(
            "gpt-5.5",
            ModelRequest {
                messages: &[Message::user_text("find current docs")],
                tools: &[ToolSpec {
                    name: "web_search".into(),
                    description: "search the web".into(),
                    input_schema: json!({"type": "object"}),
                }],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
        )
        .unwrap();

        assert_eq!(
            body["tools"],
            json!([{"type": "web_search", "external_web_access": true}])
        );
        assert_eq!(body["tool_choice"], "auto");
    }

    #[test]
    fn compact_body_omits_stream_tools_and_tool_policy_fields() {
        let tools = [ToolSpec {
            name: "bash".into(),
            description: "run a command".into(),
            input_schema: json!({"type": "object"}),
        }];
        let request = ModelRequest {
            messages: &[
                Message::System("be helpful".into()),
                Message::user_text("hello"),
            ],
            tools: &tools,
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: Some("session-1"),
        };

        let standard = ResponsesProfile::from_auth(&Auth::ApiKey("key".into()), "gpt-5.4");
        let standard_body = build_responses_compact_body(
            &standard,
            &OpenAiReasoningProfile::unknown(),
            request.clone(),
        )
        .unwrap();
        assert_compact_body_omits_tool_fields(&standard_body);
        assert_eq!(standard_body["prompt_cache_key"], "session-1");
        assert_eq!(standard_body["store"], false);
        assert!(standard_body.get("include").is_none());
        assert!(standard_body.get("instructions").is_some());

        let lite = ResponsesProfile::from_auth(
            &Auth::Codex {
                tokens: crate::credentials::CodexTokens {
                    access_token: "test".into(),
                    refresh_token: None,
                    id_token: None,
                    account_id: None,
                },
                source: CodexAuthSource::Env,
            },
            "gpt-5.6-sol",
        );
        let lite_body =
            build_responses_compact_body(&lite, &OpenAiReasoningProfile::unknown(), request)
                .unwrap();
        assert_compact_body_omits_tool_fields(&lite_body);
        assert!(lite_body
            .get("input")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .all(|item| item.get("type").and_then(Value::as_str) != Some("additional_tools")));
        assert_eq!(
            lite_body["reasoning"],
            json!({"effort": "medium", "summary": "auto", "context": "all_turns"})
        );
    }

    fn assert_compact_body_omits_tool_fields(body: &Value) {
        assert!(body.get("stream").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("additional_tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("parallel_tool_calls").is_none());
    }
}
