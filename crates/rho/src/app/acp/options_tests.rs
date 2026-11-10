use agent_client_protocol::{
    schema::v1::{
        AvailableCommand, AvailableCommandInput, SessionConfigOptionValue, SessionId,
        SetSessionConfigOptionRequest, UnstructuredCommandInput,
    },
    ErrorCode,
};
use pretty_assertions::assert_eq;
use rho_protocol::{self as wire, SessionChange};
use serde_json::json;

use super::{available_commands, change, config_options, modes, Toggles};

fn model_ref(provider: &str, model: &str) -> wire::ModelRef {
    wire::ModelRef {
        provider: provider.into(),
        model: model.into(),
    }
}

/// Options with every config option, one known value left out of each
/// list a newer daemon could widen.
fn options() -> wire::SessionOptions {
    wire::SessionOptions {
        model: wire::ModelOptions {
            auth: "api-key".into(),
            current: model_ref("openai", "gpt-4"),
            available: vec![
                wire::ModelChoice {
                    model: model_ref("xai", "grok-3"),
                    label: "Grok 3".into(),
                },
                wire::ModelChoice {
                    model: model_ref("openrouter", "openai/gpt-4"),
                    label: "gpt-4 via OpenRouter".into(),
                },
                wire::ModelChoice {
                    model: model_ref("openai", "gpt-4"),
                    label: "gpt-4".into(),
                },
            ],
        },
        reasoning: Some(wire::ReasoningOptions {
            current: wire::ReasoningLevel::High,
            available: vec![
                wire::ReasoningLevel::Low,
                wire::ReasoningLevel::High,
                wire::ReasoningLevel::Xhigh,
                wire::ReasoningLevel::Unknown,
            ],
        }),
        fast_mode: Some(wire::FastModeOptions { enabled: true }),
        permission_mode: wire::PermissionModeOptions {
            current: wire::PermissionMode::Plan,
            available: vec![
                wire::PermissionMode::Plan,
                wire::PermissionMode::Auto,
                wire::PermissionMode::Unknown,
            ],
        },
        commands: Vec::new(),
        goal: None,
        added_dirs: Vec::new(),
    }
}

// Covers: hosts get mode, model, thought_level and fast_mode in that order,
// each with the values a host sends back (`provider/model` ids, lowercase
// levels, on/off or a boolean as the host advertised), and nothing a newer
// daemon added that this rho cannot name
// Owner: ACP config options
#[test]
fn config_options_render_every_daemon_option_in_order() {
    let rendered = |toggles| serde_json::to_value(config_options(&options(), toggles)).unwrap();
    let select = rendered(Toggles::Select);
    assert_eq!(
        select,
        json!([
            {
                "id": "mode", "name": "Mode", "category": "mode",
                "description": "Which tool calls run without asking.",
                "type": "select", "currentValue": "plan",
                "options": [
                    {"value": "plan", "name": "Plan"},
                    {"value": "auto", "name": "Auto"},
                ],
            },
            {
                "id": "model", "name": "Model", "category": "model",
                "type": "select", "currentValue": "openai/gpt-4",
                "options": [
                    {"value": "xai/grok-3", "name": "Grok 3", "description": "xai/grok-3"},
                    {
                        "value": "openrouter/openai/gpt-4",
                        "name": "gpt-4 via OpenRouter",
                        "description": "openrouter/openai/gpt-4",
                    },
                    {"value": "openai/gpt-4", "name": "gpt-4", "description": "openai/gpt-4"},
                ],
            },
            {
                "id": "thought_level", "name": "Reasoning", "category": "thought_level",
                "description": "How much reasoning the model spends on each turn.",
                "type": "select", "currentValue": "high",
                "options": [
                    {"value": "low", "name": "Low"},
                    {"value": "high", "name": "High"},
                    {"value": "xhigh", "name": "Extra high"},
                ],
            },
            {
                "id": "fast_mode", "name": "Fast mode", "category": "model_config",
                "description": "Faster responses at a higher credit rate.",
                "type": "select", "currentValue": "on",
                "options": [
                    {"value": "off", "name": "Normal"},
                    {"value": "on", "name": "Fast"},
                ],
            },
        ])
    );
    assert_eq!(
        rendered(Toggles::Boolean)[3],
        json!({
            "id": "fast_mode", "name": "Fast mode", "category": "model_config",
            "description": "Faster responses at a higher credit rate.",
            "type": "boolean", "currentValue": true,
        })
    );
}

// Covers: a model without reasoning or fast mode, or a mode this rho does
// not know as the current one, leaves those options out entirely
// Owner: ACP config options
#[test]
fn options_the_session_lacks_are_left_out() {
    let mut options = options();
    options.reasoning = None;
    options.fast_mode = None;
    options.permission_mode.current = wire::PermissionMode::Unknown;
    let ids = config_options(&options, Toggles::Select)
        .into_iter()
        .map(|option| option.id.0.to_string())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["model"]);
    assert_eq!(modes(&options.permission_mode), None);
}

// Covers: the legacy `modes` field lists the known modes with the ids the
// `mode` option uses
// Owner: ACP config options
#[test]
fn modes_list_known_permission_modes() {
    let modes = serde_json::to_value(modes(&options().permission_mode)).unwrap();
    assert_eq!(
        modes,
        json!({
            "currentModeId": "plan",
            "availableModes": [
                {"id": "plan", "name": "Plan"},
                {"id": "auto", "name": "Auto"},
            ],
        })
    );
}

// Covers: each pick becomes the daemon change it names, resolved against
// what the session offers; a value it does not offer, or of the wrong
// shape, is InvalidParams
// Owner: ACP config options
#[test]
fn picks_become_daemon_changes() {
    let cases: [(
        &str,
        SessionConfigOptionValue,
        Toggles,
        Result<SessionChange, ErrorCode>,
    ); 11] = [
        (
            "mode",
            "auto".into(),
            Toggles::Select,
            Ok(SessionChange::PermissionMode {
                mode: wire::PermissionMode::Auto,
            }),
        ),
        (
            "mode",
            "sideways".into(),
            Toggles::Select,
            Err(ErrorCode::InvalidParams),
        ),
        (
            "model",
            "openrouter/openai/gpt-4".into(),
            Toggles::Select,
            Ok(SessionChange::Model {
                auth: None,
                model: model_ref("openrouter", "openai/gpt-4"),
            }),
        ),
        (
            "model",
            "anthropic/claude".into(),
            Toggles::Select,
            Err(ErrorCode::InvalidParams),
        ),
        (
            "thought_level",
            "xhigh".into(),
            Toggles::Select,
            Ok(SessionChange::Reasoning {
                level: wire::ReasoningLevel::Xhigh,
            }),
        ),
        (
            "thought_level",
            "max".into(),
            Toggles::Select,
            Err(ErrorCode::InvalidParams),
        ),
        (
            "fast_mode",
            "off".into(),
            Toggles::Select,
            Ok(SessionChange::FastMode { enabled: false }),
        ),
        (
            "fast_mode",
            SessionConfigOptionValue::boolean(true),
            Toggles::Select,
            Err(ErrorCode::InvalidParams),
        ),
        (
            "fast_mode",
            SessionConfigOptionValue::boolean(true),
            Toggles::Boolean,
            Ok(SessionChange::FastMode { enabled: true }),
        ),
        (
            "fast_mode",
            "on".into(),
            Toggles::Boolean,
            Err(ErrorCode::InvalidParams),
        ),
        (
            "verbosity",
            "high".into(),
            Toggles::Select,
            Err(ErrorCode::InvalidParams),
        ),
    ];
    for (id, value, toggles, expected) in cases {
        let request = SetSessionConfigOptionRequest::new(SessionId::new("s"), id, value);
        let picked = change(&request, &options(), toggles).map_err(|error| error.code);
        assert_eq!(picked, expected, "{id} {:?} {toggles:?}", request.value);
    }
}

// Covers: a skill is advertised as `$<skill>`, which T3 Code's `$` menu
// requires, a slash command by name with its hint, and a command of a kind
// a newer daemon sends is left out rather than shown as something it is not
// Owner: ACP commands
#[test]
fn available_commands_render_each_kind_and_skip_unknown_ones() {
    let command = |name: &str, kind, input_hint: Option<&str>| wire::SessionCommand {
        name: name.into(),
        kind,
        description: format!("{name} description"),
        input_hint: input_hint.map(str::to_owned),
    };

    let advertised = available_commands(&[
        command("prompt:review", wire::CommandKind::Slash, Some("[$1]")),
        command("demo", wire::CommandKind::Skill, None),
        command("later", wire::CommandKind::Unknown, None),
    ]);

    assert_eq!(
        advertised,
        [
            AvailableCommand::new("prompt:review", "prompt:review description").input(
                AvailableCommandInput::Unstructured(UnstructuredCommandInput::new("[$1]"))
            ),
            AvailableCommand::new("$demo", "demo description"),
        ]
    );
}
