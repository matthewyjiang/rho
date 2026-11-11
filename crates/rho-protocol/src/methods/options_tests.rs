use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::methods::SessionEvent;

// Covers: every change's wire form (the `kind` tag and snake_case names)
// round-trips, which ACP's set_mode/set_config_option and the TUI's
// controls map onto (#1670, #1667).
// Owner: protocol wire contract.
#[test]
fn every_session_change_round_trips_through_its_wire_form() {
    let cases = [
        (
            SessionChange::Model {
                model: ModelRef {
                    provider: "openai".into(),
                    model: "gpt-5.5".into(),
                },
            },
            json!({"kind": "model", "model": {"provider": "openai", "model": "gpt-5.5"}}),
        ),
        (
            SessionChange::Reasoning {
                level: ReasoningLevel::Xhigh,
            },
            json!({"kind": "reasoning", "level": "xhigh"}),
        ),
        (
            SessionChange::FastMode { enabled: true },
            json!({"kind": "fast_mode", "enabled": true}),
        ),
        (
            SessionChange::PermissionMode {
                mode: PermissionMode::AllowEdits,
            },
            json!({"kind": "permission_mode", "mode": "allow_edits"}),
        ),
    ];
    for (change, wire) in cases {
        assert_eq!(serde_json::to_value(&change).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<SessionChange>(wire).unwrap(),
            change
        );
    }
}

// Covers: a session's options travel whole in `options_changed`, with the
// per-model reasoning and fast mode left out when the model has neither,
// and a command's hint left out when it takes nothing.
// Owner: protocol wire contract.
#[test]
fn options_changed_leaves_out_what_the_model_lacks() {
    let gpt = ModelRef {
        provider: "openai".into(),
        model: "gpt-5.5".into(),
    };
    let event = SessionEvent::OptionsChanged {
        options: Box::new(SessionOptions {
            model: ModelOptions {
                auth: "api-key".into(),
                current: gpt.clone(),
                available: vec![ModelChoice {
                    model: gpt,
                    label: "GPT-5.5".into(),
                }],
            },
            reasoning: None,
            fast_mode: None,
            permission_mode: PermissionModeOptions {
                current: PermissionMode::Plan,
                available: vec![PermissionMode::Plan, PermissionMode::Supervised],
            },
            commands: vec![
                SessionCommand {
                    name: "init".into(),
                    kind: CommandKind::Slash,
                    description: "Set up AGENTS.md".into(),
                    input_hint: None,
                },
                SessionCommand {
                    name: "mcp:docs:search".into(),
                    kind: CommandKind::Slash,
                    description: "Search the docs".into(),
                    input_hint: Some("<query>".into()),
                },
            ],
            goal: None,
        }),
    };
    let wire = json!({
        "type": "options_changed",
        "options": {
            "model": {
                "current": {"provider": "openai", "model": "gpt-5.5"},
                "auth": "api-key",
                "available": [{"model": {"provider": "openai", "model": "gpt-5.5"}, "label": "GPT-5.5"}],
            },
            "permission_mode": {"current": "plan", "available": ["plan", "supervised"]},
            "commands": [
                {"name": "init", "kind": "slash", "description": "Set up AGENTS.md"},
                {"name": "mcp:docs:search", "kind": "slash", "description": "Search the docs", "input_hint": "<query>"},
            ],
        },
    });
    assert_eq!(serde_json::to_value(&event).unwrap(), wire);
    assert_eq!(serde_json::from_value::<SessionEvent>(wire).unwrap(), event);
}
