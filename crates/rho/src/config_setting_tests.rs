use pretty_assertions::assert_eq;
use rho_protocol::ModelRuntime;

use super::{
    any_value, load_settings, save_settings, unknown_keys, AnySetting, Apply, ConfigDocument,
    ConfigError, IntegerSpec, ModelSetting, ModelSpec, Setting, SettingMeta,
};
use crate::permission::PermissionMode;

#[derive(Clone, Debug, Default, PartialEq)]
struct Store {
    flag: bool,
    count: u64,
    mode: PermissionMode,
    endpoint: Option<String>,
    agent: Option<ModelSetting>,
}

const META: SettingMeta = SettingMeta {
    category: "test",
    section: None,
    label: "Test",
    help: "Test.",
    applies: Apply::Immediately,
    overridable_per_project: false,
};

static FLAG: Setting<Store, bool> = Setting {
    key: "group.flag",
    meta: META,
    spec: (),
    get: |store| store.flag,
    set: |store, flag| store.flag = flag,
    validate: any_value,
};

static COUNT: Setting<Store, u64> = Setting {
    key: "group.count",
    meta: META,
    spec: IntegerSpec {
        min: 2,
        max: Some(9),
        unit: None,
    },
    get: |store| store.count,
    set: |store, count| store.count = count,
    validate: any_value,
};

static MODE: Setting<Store, PermissionMode> = Setting {
    key: "mode",
    meta: META,
    spec: (),
    get: |store| store.mode,
    set: |store, mode| store.mode = mode,
    validate: any_value,
};

static ENDPOINT: Setting<Store, Option<String>> = Setting {
    key: "group.endpoint",
    meta: META,
    spec: (),
    get: |store| store.endpoint.clone(),
    set: |store, endpoint| store.endpoint = endpoint,
    validate: |endpoint| match endpoint {
        Some(url) if !url.starts_with("https://") => Err("must use https".into()),
        _ => Ok(()),
    },
};

static AGENT: Setting<Store, Option<ModelSetting>> = Setting {
    key: "agents.helper",
    meta: META,
    spec: ModelSpec {
        runtimes: &[ModelRuntime::Rho, ModelRuntime::ClaudeCli],
    },
    get: |store| store.agent.clone(),
    set: |store, agent| store.agent = agent,
    validate: any_value,
};

static SETTINGS: &[&dyn AnySetting<Store>] = &[&FLAG, &COUNT, &MODE, &ENDPOINT, &AGENT];

fn load(text: &str) -> Result<Store, Vec<ConfigError>> {
    let doc = ConfigDocument::parse(text).unwrap();
    let mut store = Store::default();
    let mut errors = unknown_keys(&doc, SETTINGS, &["tables"]);
    load_settings(SETTINGS, &doc, &mut store, &mut errors);
    if errors.is_empty() {
        Ok(store)
    } else {
        Err(errors)
    }
}

// Covers: each value type rejects a wrong value with the key and what it
// got, instead of falling back to the default.
// Owner: config settings
#[test]
fn bad_values_are_errors_at_their_key() {
    for (text, key, message) in [
        (
            "group.flag = 1",
            "group.flag",
            "must be true or false; got 1",
        ),
        (
            "group.count = 1",
            "group.count",
            "must be at least 2; got 1",
        ),
        (
            "group.count = 10",
            "group.count",
            "must be at most 9; got 10",
        ),
        (
            "group.count = 2.5",
            "group.count",
            "must be a whole number; got 2.5",
        ),
        (
            "mode = \"ask\"",
            "mode",
            "must be one of bypass, auto, allow_edits, plan, supervised; got \"ask\"",
        ),
        (
            "group.endpoint = \"http://x\"",
            "group.endpoint",
            "must use https",
        ),
        (
            "agents.helper = \"gpt-5.5\"",
            "agents.helper",
            "must be a table with provider and model keys",
        ),
        (
            "[agents.helper]\nruntime = \"claude-cli\"\nprovider = \"anthropic\"",
            "agents.helper",
            "runtime claude-cli takes no provider or auth",
        ),
        (
            "[agents.helper]\nruntime = \"codex\"",
            "agents.helper",
            "runtime must be one of rho, claude-cli; got \"codex\"",
        ),
    ] {
        assert_eq!(
            load(text),
            Err(vec![ConfigError::at(key, message)]),
            "{text}"
        );
    }
}

// Covers: keys no setting owns are reported, while model leaves and
// file-only tables are not.
// Owner: config settings
#[test]
fn unknown_keys_skip_owned_leaves_and_tables() {
    let errors = load(
        "[agents.helper]\nprovider = \"openai\"\nmodel = \"gpt-5.5\"\nextra = 1\n[tables.anything]\ndeep.key = true\n[group]\nflg = true\n",
    )
    .unwrap_err();
    assert_eq!(
        errors.iter().map(ToString::to_string).collect::<Vec<_>>(),
        [
            "unknown key `agents.helper.extra`",
            "unknown key `group.flg`"
        ]
    );
}

// Covers: saving writes only the settings whose value changed, and an
// unset optional value removes its key.
// Owner: config settings
#[test]
fn save_writes_only_changed_settings() {
    let before =
        load("mode = \"plan\" # chosen\n[group]\nendpoint = \"https://x.example\"\ncount = 5\n")
            .unwrap();
    let after = Store {
        count: 6,
        endpoint: None,
        ..before.clone()
    };
    let mut doc = ConfigDocument::parse(
        "mode = \"plan\" # chosen\n[group]\nendpoint = \"https://x.example\"\ncount = 5\n",
    )
    .unwrap();

    save_settings(SETTINGS, &before, &before, &mut doc);
    save_settings(SETTINGS, &before, &after, &mut doc);

    assert_eq!(doc.text(), "mode = \"plan\" # chosen\n[group]\ncount = 6\n");
}
