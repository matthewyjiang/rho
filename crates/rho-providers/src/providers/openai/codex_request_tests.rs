use super::*;
use crate::model::Message;

#[tokio::test]
async fn priority_service_tier_is_sent_as_fast_mode() {
    let body = build_codex_responses_body_with_tier(
        "gpt-5.5",
        ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
        Some(ServiceTier::Priority),
        /*hosted_web_search*/ true,
    )
    .await
    .unwrap();

    assert_eq!(body["service_tier"], "priority");
}

#[tokio::test]
async fn priority_service_tier_is_omitted_for_unsupported_codex_models() {
    let body = build_codex_responses_body_with_tier(
        "gpt-5.3-codex-spark",
        ModelRequest {
            messages: &[Message::user_text("hello")],
            tools: &[],
            cancellation: Default::default(),
            reasoning_level: Default::default(),
            prompt_cache_key: None,
        },
        Some(ServiceTier::Priority),
        /*hosted_web_search*/ true,
    )
    .await
    .unwrap();

    assert!(body.get("service_tier").is_none());
}

#[tokio::test]
async fn priority_service_tier_is_limited_to_codex_auth() {
    let request = ModelRequest {
        messages: &[Message::user_text("hello")],
        tools: &[],
        cancellation: Default::default(),
        reasoning_level: Default::default(),
        prompt_cache_key: None,
    };
    let profile = ResponsesProfile::from_auth(&Auth::ApiKey("key".into()), "gpt-5.5");

    let body = build_responses_create_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        request,
        Some(ServiceTier::Priority),
        /*hosted_web_search*/ true,
    )
    .await
    .unwrap();

    assert!(body.get("service_tier").is_none());
}

#[tokio::test]
async fn responses_lite_sets_all_turns_reasoning_context() {
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
    .await
    .unwrap();

    assert_eq!(
        body["reasoning"],
        json!({"effort": "medium", "summary": "auto", "context": "all_turns"})
    );
}

#[tokio::test]
async fn responses_lite_moves_tools_and_instructions_into_input() {
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
    .await
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
                "strict": null,
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

#[tokio::test]
async fn standard_requests_keep_hosted_web_search_tool() {
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
    .await
    .unwrap();

    assert_eq!(
        body["tools"],
        json!([{"type": "web_search", "external_web_access": true}])
    );
    assert_eq!(body["tool_choice"], "auto");
}

#[tokio::test]
async fn standard_requests_keep_function_web_search_when_hosted_disabled() {
    let body = build_codex_responses_body_with_tier(
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
        None,
        /*hosted_web_search*/ false,
    )
    .await
    .unwrap();

    assert_eq!(
        body["tools"],
        json!([{
            "type": "function",
            "name": "web_search",
            "description": "search the web",
            "parameters": {"type": "object"},
            "strict": null,
        }])
    );
}

// Covers: Codex and API-key requests must keep their distinct Responses wire contracts.
// Owner: OpenAI Responses request lowering
#[tokio::test]
async fn standard_create_wire_contract_is_auth_flavor_specific() {
    struct Case {
        name: &'static str,
        auth: Auth,
        expected_strict: Value,
        expected_parallel_tool_calls: Option<Value>,
    }

    let cases = [
        Case {
            name: "api key",
            auth: Auth::ApiKey("key".into()),
            expected_strict: json!(false),
            expected_parallel_tool_calls: None,
        },
        Case {
            name: "Codex",
            auth: codex_test_auth(),
            expected_strict: Value::Null,
            expected_parallel_tool_calls: Some(json!(true)),
        },
    ];
    let messages = [Message::user_text("hello")];
    let tools = [ToolSpec {
        name: "bash".into(),
        description: "run a command".into(),
        input_schema: json!({"type": "object"}),
    }];
    let expected_include = json!(["reasoning.encrypted_content"]);

    for case in cases {
        let profile = ResponsesProfile::from_auth(&case.auth, "gpt-5.4");
        let body = build_responses_create_body(
            &profile,
            &OpenAiReasoningProfile::unknown(),
            ModelRequest {
                messages: &messages,
                tools: &tools,
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
            None,
            /*hosted_web_search*/ true,
        )
        .await
        .unwrap();

        assert_eq!(
            body["tools"][0].get("strict").cloned(),
            Some(case.expected_strict),
            "{} function tool strictness",
            case.name
        );
        assert_eq!(
            body.get("include"),
            Some(&expected_include),
            "{} reasoning include",
            case.name
        );
        assert_eq!(
            body.get("parallel_tool_calls").cloned(),
            case.expected_parallel_tool_calls,
            "{} parallel tool policy",
            case.name
        );
    }
}

#[tokio::test]
async fn compact_body_omits_stream_tools_and_tool_policy_fields() {
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
    .await
    .unwrap();
    assert_compact_body_omits_tool_fields(&standard_body);
    assert_eq!(standard_body["prompt_cache_key"], "session-1");
    assert_eq!(standard_body["store"], false);
    assert!(standard_body.get("include").is_none());
    assert!(standard_body.get("instructions").is_some());

    let lite = ResponsesProfile::from_auth(&codex_test_auth(), "gpt-5.6-sol");
    let lite_body =
        build_responses_compact_body(&lite, &OpenAiReasoningProfile::unknown(), request)
            .await
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
