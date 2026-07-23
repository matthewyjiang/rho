use serde_json::{json, Value};

use crate::model::{ModelError, ModelRequest};
use rho_tools::tool::ToolSpec;

use crate::protocol::openai_responses::{
    codex_input_items_for_target, codex_reasoning_param, to_responses_lite_tool, to_responses_tool,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CodexRequestMode {
    Standard,
    ResponsesLite,
}

impl CodexRequestMode {
    pub(super) fn for_model(model: &str) -> Self {
        match model {
            "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna" => Self::ResponsesLite,
            _ => Self::Standard,
        }
    }

    pub(super) fn uses_responses_lite(self) -> bool {
        match self {
            Self::Standard => false,
            Self::ResponsesLite => true,
        }
    }

    /// Rho does not yet retain server output items in its continuation baseline.
    /// Responses Lite tool turns therefore use full request bodies so the
    /// model's previous function call is not duplicated in the next delta.
    pub(super) fn supports_incremental_websocket(self) -> bool {
        match self {
            Self::Standard => true,
            Self::ResponsesLite => false,
        }
    }
}

pub(super) fn build_codex_responses_body_with_profile(
    model: &str,
    reasoning_profile: &super::reasoning::OpenAiReasoningProfile,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    let target = crate::model::ModelIdentity::new("openai-codex", "openai-responses", model);
    build_responses_body_with_profile(
        "openai-codex",
        model,
        &target,
        reasoning_profile,
        request,
        CodexRequestMode::for_model(model),
    )
}

/// Builds a Responses API body for Codex or direct OpenAI API-key turns.
pub(super) fn build_responses_body_with_profile(
    provider: &'static str,
    model: &str,
    target: &crate::model::ModelIdentity,
    reasoning_profile: &super::reasoning::OpenAiReasoningProfile,
    request: ModelRequest<'_>,
    mode: CodexRequestMode,
) -> Result<Value, ModelError> {
    let reasoning = reasoning_profile.config(provider, model, request.reasoning_level)?;
    let mut instructions = Vec::new();
    let mut input =
        codex_input_items_for_target(request.messages.to_vec(), &mut instructions, Some(target))?;
    let tools = request
        .tools
        .iter()
        .map(|tool| responses_tool(mode, tool.clone()))
        .collect::<Vec<_>>();
    let instructions = instructions.join("\n\n");
    let mut body = json!({
        "model": model,
        "store": false,
        "stream": true
    });

    match mode {
        CodexRequestMode::Standard => {
            body["instructions"] = json!(instructions);
            body["input"] = json!(input);
            if !tools.is_empty() {
                body["tools"] = json!(tools);
                body["tool_choice"] = json!("auto");
            }
        }
        CodexRequestMode::ResponsesLite => {
            input.insert(
                0,
                json!({
                    "type": "additional_tools",
                    "role": "developer",
                    "tools": tools,
                }),
            );
            if !instructions.is_empty() {
                input.insert(
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
            body["input"] = json!(input);
            body["tool_choice"] = json!("auto");
            body["parallel_tool_calls"] = json!(false);
        }
    }

    if let Some(prompt_cache_key) = request.prompt_cache_key {
        body["prompt_cache_key"] = json!(prompt_cache_key);
    }
    if let Some(mut reasoning) =
        codex_reasoning_param(reasoning.effort.as_deref(), reasoning.summary.as_deref())
    {
        if mode == CodexRequestMode::ResponsesLite {
            reasoning["context"] = json!("all_turns");
        }
        body["reasoning"] = reasoning;
    }

    Ok(body)
}

fn responses_tool(mode: CodexRequestMode, tool: ToolSpec) -> Value {
    match mode {
        CodexRequestMode::Standard => to_responses_tool(tool),
        CodexRequestMode::ResponsesLite => to_responses_lite_tool(tool),
    }
}

#[cfg(test)]
pub(super) fn build_codex_responses_body(
    model: &str,
    request: ModelRequest<'_>,
) -> Result<Value, ModelError> {
    build_codex_responses_body_with_profile(
        model,
        &super::reasoning::OpenAiReasoningProfile::unknown(),
        request,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Message;

    #[test]
    fn gpt_5_6_models_use_responses_lite_without_incremental_websocket_continuation() {
        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let mode = CodexRequestMode::for_model(model);
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

            assert_eq!(mode, CodexRequestMode::ResponsesLite, "{model}");
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
}
