use std::collections::BTreeSet;

use super::*;
use crate::model::Message;

#[tokio::test]
async fn priority_service_tier_is_sent_as_fast_mode() {
    for model in ["gpt-5.5", "gpt-6-astra"] {
        let body = build_codex_responses_body_with_tier(
            model,
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
        .unwrap();

        assert_eq!(body["service_tier"], "priority", "{model}");
    }
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
        &BTreeSet::new(),
    )
    .unwrap()
    .body;

    assert!(body.get("service_tier").is_none());
}

#[tokio::test]
async fn gpt56_codex_models_use_standard_wire_contract() {
    for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
        let profile = ResponsesProfile::from_auth(&codex_test_auth(), model);
        assert_eq!(profile.contract(), ResponsesWireContract::CodexStandard);

        let body = build_codex_responses_body(
            model,
            ModelRequest {
                messages: &[
                    Message::System("follow the repository instructions".into()),
                    Message::user_text("hello"),
                ],
                tools: &[ToolSpec {
                    name: "bash".into(),
                    description: "run a command".into(),
                    input_schema: json!({"type": "object"}),
                }],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
        )
        .unwrap();

        assert_eq!(
            body["instructions"], "follow the repository instructions",
            "{model} keeps instructions top-level"
        );
        assert_eq!(
            body["parallel_tool_calls"], true,
            "{model} enables parallel tools"
        );
        assert_eq!(
            body.get("include"),
            Some(&json!(["reasoning.encrypted_content"])),
            "{model} includes encrypted reasoning"
        );
        assert!(
            body.get("reasoning")
                .and_then(|value| value.get("context"))
                .is_none(),
            "{model} does not set lite reasoning context"
        );
        assert!(body["tools"].is_array(), "{model} keeps tools top-level");
        assert!(
            body["input"]
                .as_array()
                .expect("standard contract must serialize input as an array")
                .iter()
                .all(|item| item.get("type").and_then(Value::as_str) != Some("additional_tools")),
            "{model} does not nest tools in input"
        );
    }
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
        /* service_tier */ None,
        /*hosted_web_search*/ false,
    )
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
            /* service_tier */ None,
            /*hosted_web_search*/ true,
            &BTreeSet::new(),
        )
        .unwrap()
        .body;

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
    .unwrap();
    assert_compact_body_omits_tool_fields(&standard_body);
    assert_eq!(standard_body["prompt_cache_key"], "session-1");
    assert_eq!(standard_body["store"], false);
    assert!(standard_body.get("include").is_none());
    assert!(standard_body.get("instructions").is_some());

    let codex = ResponsesProfile::from_auth(&codex_test_auth(), "gpt-5.6-sol");
    let codex_body =
        build_responses_compact_body(&codex, &OpenAiReasoningProfile::unknown(), request).unwrap();
    assert_compact_body_omits_tool_fields(&codex_body);
    assert!(codex_body.get("instructions").is_some());
    assert!(codex_body
        .get("reasoning")
        .and_then(|value| value.get("context"))
        .is_none());
}

fn assert_compact_body_omits_tool_fields(body: &Value) {
    assert!(body.get("stream").is_none());
    assert!(body.get("tools").is_none());
    assert!(body.get("additional_tools").is_none());
    assert!(body.get("tool_choice").is_none());
    assert!(body.get("parallel_tool_calls").is_none());
}

#[test]
fn create_and_compact_body_builders_diverge_on_tools() {
    let profile = ResponsesProfile::from_auth(&Auth::ApiKey("key".into()), "gpt-5.4");
    let request = ModelRequest {
        messages: &[Message::user_text("hello")],
        tools: &[crate::model::ToolSpec {
            name: "bash".into(),
            description: "run".into(),
            input_schema: json!({"type":"object"}),
        }],
        cancellation: Default::default(),
        reasoning_level: Default::default(),
        prompt_cache_key: None,
    };
    let create = build_responses_create_body(
        &profile,
        &OpenAiReasoningProfile::unknown(),
        request.clone(),
        None,
        /*hosted_web_search*/ true,
        &BTreeSet::new(),
    )
    .unwrap()
    .body;
    let compact =
        build_responses_compact_body(&profile, &OpenAiReasoningProfile::unknown(), request)
            .unwrap();
    assert_eq!(create["stream"], true);
    assert!(create.get("tools").is_some());
    assert!(compact.get("stream").is_none());
    assert!(compact.get("tools").is_none());
    assert!(compact.get("tool_choice").is_none());
    assert!(compact.get("parallel_tool_calls").is_none());
}

// Covers: `"async": true` is advertised only for Astra plus a declared function
// tool; hosted web_search never gets the flag.
// Owner: OpenAI Responses request lowering
#[test]
fn async_tools_are_advertised_only_when_supported_and_declared() {
    struct Case {
        name: &'static str,
        model: &'static str,
        tool_name: &'static str,
        async_tools: BTreeSet<String>,
        hosted_web_search: bool,
        expect_async: bool,
        expect_hosted: bool,
    }
    let function = |name: &str| ToolSpec {
        name: name.into(),
        description: "run".into(),
        input_schema: json!({"type": "object"}),
    };
    let cases = [
        Case {
            name: "astra plus declared",
            model: "gpt-6-astra",
            tool_name: "one_agent",
            async_tools: BTreeSet::from(["one_agent".into()]),
            hosted_web_search: true,
            expect_async: true,
            expect_hosted: false,
        },
        Case {
            name: "astra plus undeclared",
            model: "gpt-6-astra",
            tool_name: "one_agent",
            async_tools: BTreeSet::new(),
            hosted_web_search: true,
            expect_async: false,
            expect_hosted: false,
        },
        Case {
            name: "gpt-5.5 plus declared",
            model: "gpt-5.5",
            tool_name: "one_agent",
            async_tools: BTreeSet::from(["one_agent".into()]),
            hosted_web_search: true,
            expect_async: false,
            expect_hosted: false,
        },
        Case {
            name: "hosted web_search never",
            model: "gpt-6-astra",
            tool_name: "web_search",
            async_tools: BTreeSet::from(["web_search".into()]),
            hosted_web_search: true,
            expect_async: false,
            expect_hosted: true,
        },
    ];

    for case in cases {
        let profile = ResponsesProfile::from_auth(&codex_test_auth(), case.model);
        let tools = [function(case.tool_name)];
        let body = build_responses_create_body(
            &profile,
            &OpenAiReasoningProfile::unknown(),
            ModelRequest {
                messages: &[Message::user_text("hello")],
                tools: &tools,
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            },
            None,
            case.hosted_web_search,
            &case.async_tools,
        )
        .unwrap()
        .body;
        let tool = &body["tools"][0];
        if case.expect_hosted {
            assert_eq!(tool["type"], "web_search", "{}", case.name);
        } else {
            assert_eq!(tool["type"], "function", "{}", case.name);
            assert_eq!(tool["name"], case.tool_name, "{}", case.name);
        }
        assert_eq!(
            tool.get("async") == Some(&json!(true)),
            case.expect_async,
            "{}",
            case.name
        );
    }
}
