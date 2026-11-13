use pretty_assertions::assert_eq;

use super::*;

const OPUS: &str = "cliproxyapi/anthropic/claude-opus-5-5";

/// Trimmed from T3 Code's reply to `orchestrator_capabilities {"search": OPUS}`
/// for a parent thread on another model of the same Rho provider instance.
fn capabilities() -> Value {
    json!({
        "parentThreadId": "b38c9b61-c866-4343-bc56-8c2042043db2",
        "inheritedProviderInstanceId": "acpRegistry_rho",
        "inheritedModel": "cliproxyapi/anthropic/claude-fable-5-2",
        "inheritedOptions": [
            {"id": "mode", "value": "bypass"},
            {"id": "thought_level", "value": "high"},
        ],
        "runtimeMode": "full-access",
        "providers": [
            {
                "providerInstanceId": "claudeAgent",
                "driverKind": "claudeAgent",
                "models": [{"id": "claude-opus-5-5", "label": "Claude Opus 5.5"}],
                "canRunChildTask": false,
                "constraints": ["Provider instance is disabled."],
            },
            {
                "providerInstanceId": "acpRegistry_rho",
                "driverKind": "acpRegistry",
                "displayName": "Rho",
                "models": [{
                    "id": OPUS,
                    "label": "Claude Opus 5.5",
                    "options": [
                        {
                            "id": "mode",
                            "values": ["bypass", "auto", "allow_edits", "plan", "supervised"],
                            "default": "bypass",
                        },
                        {
                            "id": "thought_level",
                            "values": ["low", "medium", "high", "xhigh", "max"],
                            "default": "high",
                        },
                        {"id": "fast_mode", "values": [false, true], "default": false},
                    ],
                }],
                "canRunChildTask": true,
                "constraints": [],
            },
        ],
    })
}

// Covers: a target must never change the launch it labels. T3 fails a launch
// whose model the parent's provider instance does not offer, and drops the
// parent's permission mode and thought level unless the target repeats them.
// Owner: host launch target policy.
#[test]
fn target_is_sent_only_when_it_keeps_the_launch_unchanged() {
    let inherited = |options: Value| {
        let mut capabilities = capabilities();
        capabilities["inheritedOptions"] = options;
        capabilities
    };
    for (case, capabilities, model, expected) in [
        (
            "offered model repeats the parent's options",
            capabilities(),
            OPUS,
            Some(json!({
                "model": OPUS,
                "options": [
                    {"id": "mode", "value": "bypass"},
                    {"id": "thought_level", "value": "high"},
                ],
            })),
        ),
        (
            "model offered only by another provider instance",
            capabilities(),
            "claude-opus-5-5",
            None,
        ),
        (
            "model does not accept the parent's option value",
            inherited(json!([{"id": "thought_level", "value": "ultra"}])),
            OPUS,
            None,
        ),
        (
            "model lacks one of the parent's options",
            inherited(json!([{"id": "sandbox", "value": "strict"}])),
            OPUS,
            None,
        ),
        (
            "inherited provider instance is absent from providers",
            {
                let mut capabilities = capabilities();
                capabilities["inheritedProviderInstanceId"] = json!("no-such-instance");
                capabilities
            },
            OPUS,
            None,
        ),
        (
            // Fail closed: a missing field must not read as "no options" and
            // send an empty options that drops the parent's mode/thought level.
            "reply omits inheritedOptions entirely",
            {
                let mut capabilities = capabilities();
                capabilities
                    .as_object_mut()
                    .unwrap()
                    .remove("inheritedOptions");
                capabilities
            },
            OPUS,
            None,
        ),
        (
            "reply without the expected fields",
            json!({"providers": []}),
            OPUS,
            None,
        ),
    ] {
        assert_eq!(launch_target(&capabilities, model), expected, "{case}");
    }
}
