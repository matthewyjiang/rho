use pretty_assertions::assert_eq;
use rho_protocol::{
    ModelSelection, SettingChange, SettingChoice, SettingKey, SettingKind, SettingSpec,
    SettingValue, ValueSource,
};

use super::{agent_values, apply_agent_changes, client_specs_problem};
use crate::config::{document::ConfigDocument, load::resolve};

fn set(key: &str, value: SettingValue) -> SettingChange {
    SettingChange::Set {
        key: SettingKey::new(key),
        value,
    }
}

fn reset(key: &str) -> SettingChange {
    SettingChange::Reset {
        key: SettingKey::new(key),
    }
}

// Covers: a protocol change lands in `config.toml` through its typed
// setting, keeping the rest of the file; a value the setting refuses, or a
// key no setting has, is an error naming the key, the limit, and the
// value, and the file is left as it was; a model on another provider takes
// that provider's default auth; resetting an internal agent's model drops
// its whole table, reasoning included, so the agent falls back as a fresh
// file would; its reasoning cannot be set while it has no model.
// Owner: config wire changes.
#[test]
fn agent_changes_write_by_setting_or_name_what_they_refuse() {
    let head = "# mine\n[behavior]\nagent_concurrency = 4 # slots\nrtk = true\n\n";
    let model = "[model]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nauth = \"api-key\"\n";
    let advisor = "\n[internal_agents.advisor]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nauth = \"api-key\"\nreasoning = \"high\"\n";
    let before = format!("{head}{model}{advisor}");
    /// The file after the changes, or each error that refused them.
    type Written = Result<String, Vec<&'static str>>;
    let cases: Vec<(&str, Vec<SettingChange>, Written)> = vec![
        (
            "a whole number replaces the value and keeps its comment",
            vec![set("behavior.agent_concurrency", SettingValue::Integer(8))],
            Ok(before.replace("= 4 #", "= 8 #")),
        ),
        (
            "a reset removes the key",
            vec![reset("behavior.rtk")],
            Ok(before.replace("rtk = true\n", "")),
        ),
        (
            "another provider takes its default auth",
            vec![set(
                "model",
                SettingValue::Model(ModelSelection::Rho {
                    provider: "anthropic".into(),
                    model: "claude-opus-4-8".into(),
                }),
            )],
            Ok(format!("{head}[model]\nprovider = \"anthropic\"\nmodel = \"claude-opus-4-8\"\nauth = \"anthropic-api-key\"\n{advisor}")),
        ),
        (
            "resetting an internal agent's model drops its reasoning too",
            vec![reset("internal_agents.advisor")],
            Ok(format!("{head}{model}")),
        ),
        (
            "an internal agent's reasoning needs its model",
            vec![
                reset("internal_agents.advisor"),
                set("internal_agents.advisor.reasoning", SettingValue::Text("low".into())),
            ],
            Err(vec![
                "`internal_agents.advisor.reasoning`: cannot hold a value until the setting it belongs to is set",
            ]),
        ),
        (
            "every refused change is named",
            vec![
                set("behavior.agent_concurrency", SettingValue::Integer(65)),
                set("behavior.permission_mode", SettingValue::Text("yolo".into())),
                set("behavior.rtk", SettingValue::Integer(1)),
                set("display.zen_mode", SettingValue::Bool(true)),
            ],
            Err(vec![
                "`behavior.agent_concurrency`: must be at most 64; got 65",
                "`behavior.permission_mode`: must be one of bypass, auto, allow_edits, plan, supervised; got \"yolo\"",
                "`behavior.rtk`: takes true or false",
                "`display.zen_mode`: no such setting",
            ]),
        ),
    ];
    for (name, changes, expected) in cases {
        let mut doc = ConfigDocument::parse(&before).unwrap();
        let written = apply_agent_changes(&mut doc, &changes)
            .map(|_| doc.text())
            .map_err(|errors| errors.iter().map(ToString::to_string).collect::<Vec<_>>());
        assert_eq!(
            written,
            expected.map_err(|errors| errors.into_iter().map(str::to_owned).collect()),
            "{name}"
        );
        if written.is_err() {
            assert_eq!(doc.text(), before, "{name} leaves the file as it was");
        }
    }
}

// Covers: `get` marks a value the file sets as global and one it leaves
// out as the default, which a settings screen shows as a reset target.
// Owner: config wire codec.
#[test]
fn agent_values_say_which_values_the_file_sets() {
    let doc = ConfigDocument::parse("[behavior]\nrtk = false\n").unwrap();
    let (config, _) = resolve(&doc).unwrap();
    let values = agent_values(&doc, &config);
    let entry = |key: &str| {
        let entry = &values[&SettingKey::new(key)];
        (entry.value.clone(), entry.source)
    };
    assert_eq!(
        [entry("behavior.rtk"), entry("behavior.enable_subagents")],
        [
            (Some(SettingValue::Bool(false)), ValueSource::Global),
            (Some(SettingValue::Bool(true)), ValueSource::Default),
        ]
    );
}

fn spec(key: &str, kind: SettingKind, default: Option<SettingValue>) -> SettingSpec {
    SettingSpec {
        key: SettingKey::new(key),
        kind,
        default,
        category: "display".into(),
        section: None,
        label: key.into(),
        help: String::new(),
        applies: rho_protocol::Applies::Immediately,
        overridable_per_project: false,
    }
}

// Covers: a client's registered settings must be storable as plain TOML
// values at keys that do not collide, with defaults their kinds take, or
// the daemon would write a file it cannot read back.
// Owner: config wire codec.
#[test]
fn client_specs_are_refused_unless_storable() {
    let choice = SettingKind::Choice {
        choices: vec![SettingChoice {
            value: "live".into(),
            label: "Live".into(),
            help: None,
        }],
    };
    let cases = [
        (
            "plain settings",
            vec![
                spec(
                    "zen_mode",
                    SettingKind::Bool,
                    Some(SettingValue::Bool(false)),
                ),
                spec(
                    "streaming",
                    choice.clone(),
                    Some(SettingValue::Text("live".into())),
                ),
            ],
            None,
        ),
        (
            "a model setting",
            vec![spec(
                "model",
                SettingKind::Model {
                    runtimes: vec![rho_protocol::ModelRuntime::Rho],
                },
                None,
            )],
            Some("setting `model` must be a bool, choice, integer, or text setting"),
        ),
        (
            "a key under another",
            vec![
                spec("display", SettingKind::Bool, None),
                spec("display.zen", SettingKind::Bool, None),
            ],
            Some("setting key `display.zen` repeats or nests another key"),
        ),
        (
            "a default the choices lack",
            vec![spec(
                "streaming",
                choice,
                Some(SettingValue::Text("off".into())),
            )],
            Some("setting `streaming` has a default its kind refuses"),
        ),
        (
            "a key that is not a bare path",
            vec![spec("a b", SettingKind::Bool, None)],
            Some("setting key `a b` must be dotted letters, digits, `_`, or `-`"),
        ),
    ];
    for (name, specs, expected) in cases {
        assert_eq!(client_specs_problem(&specs).as_deref(), expected, "{name}");
    }
}
