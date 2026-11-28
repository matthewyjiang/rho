use pretty_assertions::assert_eq;
use rho_protocol::{
    ModelRuntime, ModelSelection, SettingChange, SettingChoice, SettingKey, SettingKind,
    SettingSpec, SettingValue, ValueSource,
};

use super::{agent_values, apply_agent_changes, client_specs_problem, AgentChangeError};
use crate::config::{document::ConfigDocument, load::resolve, registry::AGENT_SETTINGS};

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
// setting, keeping the rest of the file; every key no setting has, then
// every value a setting refuses, is an error naming the key, the limit,
// and the value, and the file is left as it was; a model on another provider
// names no auth, which a session then settles on the auth with credentials,
// while one on the same provider keeps the auth chosen for it; resetting an internal agent's model drops
// its whole table, reasoning included, so the agent falls back as a fresh
// file would; its reasoning cannot be set while it has no model.
// Owner: config wire changes.
#[test]
fn agent_changes_write_by_setting_or_name_what_they_refuse() {
    let head = "# mine\n[behavior]\nagent_concurrency = 4 # slots\nrtk = true\n\n";
    let model = "[model]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nauth = \"api-key\"\n";
    let advisor = "\n[internal_agents.advisor]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nauth = \"api-key\"\nreasoning = \"high\"\n";
    let before = format!("{head}{model}{advisor}");
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
            "another provider names no auth",
            vec![set("model", rho_model("anthropic", "claude-opus-4-8"))],
            Ok(format!("{head}[model]\nprovider = \"anthropic\"\nmodel = \"claude-opus-4-8\"\n{advisor}")),
        ),
        (
            "the same provider keeps the chosen auth",
            vec![set("model", rho_model("openai", "gpt-5.5-mini"))],
            Ok(before.replacen("model = \"gpt-5.5\"\n", "model = \"gpt-5.5-mini\"\n", 1)),
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
                "`internal_agents.advisor.reasoning`: cannot hold that value; set the setting it belongs to first",
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
                "`display.zen_mode`: no such setting",
                "`behavior.agent_concurrency`: must be at most 64; got 65",
                "`behavior.permission_mode`: must be one of bypass, auto, allow_edits, plan, supervised; got \"yolo\"",
                "`behavior.rtk`: takes true or false",
            ]),
        ),
    ];
    for (name, changes, expected) in cases {
        assert_eq!(applied(&before, &changes), expected, "{name}");
    }
}

// Covers: a model picked in settings without an auth chosen leaves the
// auth out, so loading fills in the provider's own default (`codex` for
// `openai-codex`, not OpenAI's key) for a session to settle on the one with
// credentials, while a retired provider name such as `xai-oauth`, which
// chose its auth, keeps that choice as `auth`.
// Owner: config wire changes (model auth origin).
#[test]
fn a_model_write_keeps_only_a_chosen_auth() {
    let cases = [
        ("xai", ("xai", "grok-4.7"), None, ("xai", "xai-api-key")),
        (
            "xai-oauth",
            ("xai", "grok-4.7"),
            Some("xai-oauth"),
            ("xai", "xai-oauth"),
        ),
        (
            "xai",
            ("openai-codex", "gpt-5.6-sol"),
            None,
            ("openai-codex", "codex"),
        ),
    ];
    for (provider, (picked, model), auth, loaded) in cases {
        let before = format!("[model]\nprovider = \"{provider}\"\nmodel = \"grok-4.5\"\n");
        let auth_line = auth
            .map(|auth| format!("auth = \"{auth}\"\n"))
            .unwrap_or_default();
        let expected =
            format!("[model]\nprovider = \"{picked}\"\nmodel = \"{model}\"\n{auth_line}");
        let written = applied(&before, &[set("model", rho_model(picked, model))]).unwrap();
        let (config, _) = resolve(&ConfigDocument::parse(&written).unwrap()).unwrap();

        assert_eq!(
            (written, (config.provider.as_str(), config.auth.as_str())),
            (expected, loaded),
            "{provider} -> {picked}"
        );
    }
}

/// The file after the changes, or each error that refused them.
type Written = Result<String, Vec<&'static str>>;

/// The file `before` becomes with `changes`, or each error that refused
/// them, prefixed `file: ` when the file is to blame; a refused change
/// must leave the file as it was.
fn applied(before: &str, changes: &[SettingChange]) -> Written {
    let mut doc = ConfigDocument::parse(before).unwrap();
    let result = apply_agent_changes(&mut doc, changes);
    if result.is_err() {
        assert_eq!(doc.text(), before, "a refused change leaves the file");
    }
    result.map(|_| doc.text()).map_err(|error| {
        let (prefix, errors) = match error {
            AgentChangeError::Refused(errors) => ("", errors),
            AgentChangeError::FileInvalid(errors) => ("file: ", errors),
        };
        errors
            .iter()
            .map(|error| &*format!("{prefix}{error}").leak())
            .collect()
    })
}

fn rho_model(provider: &str, model: &str) -> SettingValue {
    SettingValue::Model(ModelSelection::Rho {
        provider: provider.into(),
        model: model.into(),
    })
}

// Covers: a change touches only what it changes: pinning the model an
// alias names writes the concrete model; setting one internal agent leaves
// another's entry, which inherits the conversation model, as written; a
// set to the default value is written, so it outlasts a later default.
// A hand edit that no longer loads is repaired by a change to the keys it
// fails on, and refused as the file's fault otherwise. Resetting an
// internal agent's model removes its whole entry, so no leftover key keeps
// it overridden. A value above what the file can store, the credential
// store, or auto permissions with no classifier model, is refused.
// Owner: config wire changes.
#[test]
fn agent_changes_touch_only_their_keys_and_can_repair_the_file() {
    let aliases = "\n[model.aliases]\nfast = \"openai/gpt-5.5\"\n";
    let judge = "[internal_agents.compaction]\nreasoning = \"high\"\n";
    let broken = "[output]\nmax_output_bytes = 0\n";
    /// A case's name, file, changes, and what `applied` returns.
    type Case = (&'static str, String, Vec<SettingChange>, Written);
    let cases: Vec<Case> = vec![
        (
            "pinning an alias's model writes the model",
            format!("[model]\nprovider = \"openai\"\nmodel = \"@fast\"\nauth = \"api-key\"\n{aliases}"),
            vec![set("model", rho_model("openai", "gpt-5.5"))],
            Ok(format!("[model]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nauth = \"api-key\"\n{aliases}")),
        ),
        (
            "another internal agent's entry stays as written",
            judge.to_owned(),
            vec![set("internal_agents.advisor", rho_model("openai", "gpt-5.5"))],
            Ok(format!("{judge}\n[internal_agents.advisor]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nauth = \"api-key\"\n")),
        ),
        (
            "an internal agent's reset leaves no entry",
            "[internal_agents.permission-classifier]\nprovider = \"openai\"\nmodel = \"gpt-5.5-mini\"\nauth = \"api-key\"\nkind = \"text\"\nallow_threshold_percent = 80\nreasoning = \"minimal\"\n".into(),
            vec![reset("internal_agents.permission-classifier")],
            Ok(String::new()),
        ),
        (
            "a set to the default is written",
            String::new(),
            vec![set("behavior.enable_subagents", SettingValue::Bool(true))],
            Ok("[behavior]\nenable_subagents = true\n".into()),
        ),
        (
            "a change to the broken key repairs the file",
            broken.to_owned(),
            vec![set("output.max_output_bytes", SettingValue::Integer(4096))],
            Ok("[output]\nmax_output_bytes = 4096\n".into()),
        ),
        (
            "a change elsewhere leaves the file to a hand edit",
            broken.to_owned(),
            vec![set("behavior.rtk", SettingValue::Bool(false))],
            Err(vec!["file: `output.max_output_bytes`: must be at least 1; got 0"]),
        ),
        (
            "a value the file cannot store is refused",
            String::new(),
            vec![set("output.max_output_bytes", SettingValue::Integer(u64::MAX))],
            Err(vec!["`output.max_output_bytes`: must be at most 9223372036854775807; got 18446744073709551615"]),
        ),
        (
            "auto needs a classifier model",
            String::new(),
            vec![set("behavior.permission_mode", SettingValue::Text("auto".into()))],
            Err(vec!["`behavior.permission_mode`: auto needs a permission classifier model; choose one first"]),
        ),
        (
            "auto with a classifier model saves",
            String::new(),
            vec![
                set("internal_agents.permission-classifier", rho_model("openai", "gpt-5.5")),
                set("behavior.permission_mode", SettingValue::Text("auto".into())),
            ],
            Ok("[behavior]\npermission_mode = \"auto\"\n\n[internal_agents.permission-classifier]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nauth = \"api-key\"\n".into()),
        ),
        (
            "the credential store is chosen elsewhere",
            String::new(),
            vec![reset("behavior.credential_store")],
            Err(vec!["`behavior.credential_store`: change it with rho/auth/choose_store"]),
        ),
    ];
    for (name, before, changes, expected) in cases {
        assert_eq!(applied(&before, &changes), expected, "{name}");
    }
}

// Covers: `get` marks a value the file sets as global and one it leaves
// out as the default, which a settings screen shows as a reset target;
// an internal agent's entry on a runtime the agent refuses still counts
// as set, so it can be reset.
// Owner: config wire codec.
#[test]
fn agent_values_say_which_values_the_file_sets() {
    let doc = ConfigDocument::parse(
        "[behavior]\nrtk = false\n\n[internal_agents.permission-classifier]\nruntime = \"claude-cli\"\nmodel = \"haiku\"\n",
    )
    .unwrap();
    let (config, _) = resolve(&doc).unwrap();
    let values = agent_values(&doc, &config);
    let entry = |key: &str| {
        let entry = &values[&SettingKey::new(key)];
        (entry.value.clone(), entry.source)
    };
    assert_eq!(
        [
            entry("behavior.rtk"),
            entry("behavior.enable_subagents"),
            entry("internal_agents.permission-classifier"),
        ],
        [
            (Some(SettingValue::Bool(false)), ValueSource::Global),
            (Some(SettingValue::Bool(true)), ValueSource::Default),
            // The loader runs the classifier on the conversation model.
            (Some(rho_model("openai", "gpt-5.5")), ValueSource::Global),
        ]
    );
}

// Covers: an internal agent's model setting offers a runtime the loader
// refuses for that agent, so a client's pick saves and then loads back as
// the conversation model.
// Owner: config wire changes.
#[test]
fn every_internal_agent_model_loads_back_as_picked() {
    let mut picked = Vec::new();
    let mut mismatched = Vec::new();
    for spec in AGENT_SETTINGS.iter().map(|setting| setting.spec()) {
        let SettingKind::Model { runtimes } = &spec.kind else {
            continue;
        };
        if !spec.key.as_str().starts_with("internal_agents.") {
            continue;
        }
        for runtime in runtimes {
            let pick = match runtime {
                ModelRuntime::Rho => rho_model("anthropic", "claude-sonnet-4-5"),
                ModelRuntime::ClaudeCli => SettingValue::Model(ModelSelection::ClaudeCli {
                    model: Some("haiku".into()),
                }),
                other => panic!("{}: no model to pick on {other:?}", spec.key),
            };
            let mut doc = ConfigDocument::parse("").unwrap();
            apply_agent_changes(&mut doc, &[set(spec.key.as_str(), pick.clone())]).unwrap();
            let (config, _) = resolve(&doc).unwrap();
            let read = agent_values(&doc, &config)[&spec.key].value.clone();
            if read != Some(pick) {
                mismatched.push(format!("{} on {runtime:?} read back {read:?}", spec.key));
            }
            picked.push((spec.key.to_string(), *runtime));
        }
    }
    assert_eq!(mismatched, Vec::<String>::new());
    // The advisor runs on Claude Code, so the check above saw that runtime.
    assert!(picked.contains(&("internal_agents.advisor".into(), ModelRuntime::ClaudeCli)));
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
        per_machine: false,
        read_only: false,
    }
}

// Covers: a client's registered settings must be storable as plain TOML
// values at keys that do not collide, with defaults their kinds take, or
// the daemon would write a file it cannot read back; and none may be
// read-only, which nothing would enforce.
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
        (
            "a read-only setting",
            vec![SettingSpec {
                read_only: true,
                ..spec("locked", SettingKind::Bool, None)
            }],
            Some(
                "setting `locked` cannot be read-only: only rho/config/set changes a client's settings",
            ),
        ),
    ];
    for (name, specs, expected) in cases {
        assert_eq!(client_specs_problem(&specs).as_deref(), expected, "{name}");
    }
}
