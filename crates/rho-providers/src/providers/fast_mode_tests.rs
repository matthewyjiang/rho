use pretty_assertions::assert_eq;

use super::{fast_serving, request_model, FastServing, GROK_4_7, GROK_4_7_BUILD_FAST};

// Covers: `/fast` is offered only for selections that implement it, and only
// xAI OAuth grok-4.7 changes the request model id.
// Owner: fast mode policy
#[test]
fn fast_serving_follows_the_provider_implementation() {
    let cases = [
        (
            "openai-codex",
            "gpt-5.5",
            "codex",
            Some(FastServing::PriorityTier),
            "gpt-5.5",
        ),
        (
            "openai-codex",
            "gpt-5.6-sol",
            "codex",
            Some(FastServing::PriorityTier),
            "gpt-5.6-sol",
        ),
        (
            "openai-codex",
            "gpt-6-astra",
            "codex",
            Some(FastServing::PriorityTier),
            "gpt-6-astra",
        ),
        (
            "openai-codex",
            "gpt-5.3-codex-spark",
            "codex",
            None,
            "gpt-5.3-codex-spark",
        ),
        ("openai", "gpt-5.5", "api-key", None, "gpt-5.5"),
        (
            "xai",
            GROK_4_7,
            "xai-oauth",
            Some(FastServing::RequestModel(GROK_4_7_BUILD_FAST)),
            GROK_4_7_BUILD_FAST,
        ),
        ("xai", GROK_4_7, "xai-api-key", None, GROK_4_7),
        ("xai", "grok-4.6", "xai-oauth", None, "grok-4.6"),
    ];

    for (provider, model, auth, expected, wire_when_fast) in cases {
        assert_eq!(
            fast_serving(provider, model, auth),
            expected,
            "{provider}/{model} auth {auth}"
        );
        assert_eq!(request_model(provider, model, auth, true), wire_when_fast);
        assert_eq!(request_model(provider, model, auth, false), model);
    }
}
