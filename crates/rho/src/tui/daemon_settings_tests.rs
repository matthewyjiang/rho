use std::collections::BTreeMap;

use pretty_assertions::assert_eq;
use rho_protocol::{
    Applies, ConfigCategory, ConfigChangedParams, ConfigError, ConfigNamespace, ConfigRevision,
    ConfigSchema, IntegerUnit, ModelChoice, ModelRef, ModelRuntime, ModelSelection, SettingChoice,
    SettingEntry, SettingKey, SettingKind, SettingSpec, SettingValue, ValueSource,
};

use super::{Applied, DaemonSettings, NamespaceValues, SettingsNamespace, SettingsRow};
use crate::tui::{picker::PickerDetail, PickerItem};

fn spec(key: &str, label: &str, kind: SettingKind, applies: Applies) -> SettingSpec {
    SettingSpec {
        key: SettingKey::new(key),
        kind,
        default: None,
        category: "agent".into(),
        section: Some("Advisor".into()),
        label: label.into(),
        help: format!("{label} help."),
        applies,
        overridable_per_project: false,
    }
}

fn entry(value: Option<SettingValue>, source: ValueSource) -> SettingEntry {
    SettingEntry { value, source }
}

fn values(
    revision: &str,
    entries: Vec<(&str, SettingEntry)>,
    errors: Vec<ConfigError>,
) -> NamespaceValues {
    NamespaceValues {
        revision: ConfigRevision::new(revision),
        values: entries
            .into_iter()
            .map(|(key, entry)| (SettingKey::new(key), entry))
            .collect(),
        errors,
    }
}

fn models() -> Vec<ModelChoice> {
    vec![ModelChoice {
        model: ModelRef {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
        },
        label: "GPT-5.5".into(),
    }]
}

/// One kind of each, as the daemon would serve them.
fn settings() -> DaemonSettings {
    let mode = spec(
        "behavior.advisor_mode",
        "Advisor mode",
        SettingKind::Bool,
        Applies::NextTurn,
    );
    let model = spec(
        "internal_agents.advisor",
        "Advisor model",
        SettingKind::Model {
            runtimes: vec![ModelRuntime::Rho, ModelRuntime::ClaudeCli],
        },
        Applies::NextTurn,
    );
    let shell = SettingSpec {
        default: Some(SettingValue::Text("bash".into())),
        ..spec(
            "behavior.inline_shell",
            "Inline shell",
            SettingKind::Choice {
                choices: vec![
                    SettingChoice {
                        value: "bash".into(),
                        label: "Bash".into(),
                        help: None,
                    },
                    SettingChoice {
                        value: "zsh".into(),
                        label: "Zsh".into(),
                        help: None,
                    },
                ],
            },
            Applies::Immediately,
        )
    };
    let threshold = SettingSpec {
        section: None,
        category: "context".into(),
        ..spec(
            "compaction.compact_threshold_percent",
            "Compact threshold",
            SettingKind::Integer {
                min: 1,
                max: Some(100),
                unit: Some(IntegerUnit::Percent),
            },
            Applies::NextTurn,
        )
    };
    let zen = SettingSpec {
        category: "display".into(),
        section: None,
        ..spec(
            "zen_mode",
            "Zen mode",
            SettingKind::Bool,
            Applies::Immediately,
        )
    };
    // The daemon changes it only through `rho/auth/choose_store`.
    let store = SettingSpec {
        category: "providers".into(),
        section: None,
        ..spec(
            "behavior.credential_store",
            "Credential store",
            SettingKind::Text,
            Applies::Restart,
        )
    };
    let schema = ConfigSchema {
        categories: vec![
            ConfigCategory {
                id: "agent".into(),
                label: "Agent".into(),
            },
            ConfigCategory {
                id: "context".into(),
                label: "Context".into(),
            },
            // No setting sits here, so the root leaves it out.
            ConfigCategory {
                id: "tools".into(),
                label: "Tools".into(),
            },
            ConfigCategory {
                id: "providers".into(),
                label: "Providers".into(),
            },
        ],
        settings: vec![mode, model, shell, threshold, store],
        file_only: Vec::new(),
    };
    let agent = values(
        "a1",
        vec![
            (
                "behavior.advisor_mode",
                entry(Some(SettingValue::Bool(true)), ValueSource::Global),
            ),
            (
                "internal_agents.advisor",
                entry(
                    Some(SettingValue::Model(ModelSelection::Rho {
                        provider: "openai".into(),
                        model: "gpt-5.5".into(),
                    })),
                    ValueSource::Global,
                ),
            ),
            (
                "behavior.inline_shell",
                entry(
                    Some(SettingValue::Text("bash".into())),
                    ValueSource::Default,
                ),
            ),
            (
                "compaction.compact_threshold_percent",
                entry(Some(SettingValue::Integer(80)), ValueSource::Global),
            ),
        ],
        vec![ConfigError {
            key: Some(SettingKey::new("compaction.compact_threshold_percent")),
            message: "must be at most 100, not 140".into(),
        }],
    );
    let tui = values(
        "t1",
        vec![(
            "zen_mode",
            entry(Some(SettingValue::Bool(false)), ValueSource::Default),
        )],
        Vec::new(),
    );
    DaemonSettings::new(schema, vec![zen], agent, tui)
}

/// What a row shows and does, compared whole.
#[derive(Debug, PartialEq)]
struct Row {
    section: Option<String>,
    label: String,
    badge: Option<String>,
    detail: Option<String>,
    does: Option<SettingsRow>,
}

fn rows(items: Vec<PickerItem>) -> Vec<Row> {
    items
        .into_iter()
        .map(|item| Row {
            section: item.section,
            label: item.label,
            badge: item.badge.map(|badge| badge.text),
            detail: match item.detail {
                Some(PickerDetail::Text(text)) => Some(text),
                _ => None,
            },
            does: SettingsRow::parse(&item.value),
        })
        .collect()
}

fn setting(namespace: SettingsNamespace, key: &str) -> Option<SettingsRow> {
    Some(SettingsRow::Setting {
        namespace,
        key: SettingKey::new(key),
    })
}

// Covers: the root lists each agent category that has settings, in schema
// order, then the `tui` client's; an empty category would open a blank page.
// The credential store, which only `rho/auth/choose_store` changes, is left
// out, and its category with it.
// Owner: daemon settings schema → `/config` rows (pure).
#[test]
fn root_rows_list_every_category_with_settings() {
    let category = |namespace, id: &str| {
        Some(SettingsRow::Category {
            namespace,
            id: id.into(),
        })
    };
    assert_eq!(
        rows(settings().category_rows()),
        vec![
            Row {
                section: Some("Settings".into()),
                label: "Agent".into(),
                badge: None,
                detail: Some("Advisor mode, Advisor model, Inline shell.".into()),
                does: category(SettingsNamespace::Agent, "agent"),
            },
            Row {
                section: Some("Settings".into()),
                label: "Context".into(),
                badge: None,
                detail: Some("Compact threshold.".into()),
                does: category(SettingsNamespace::Agent, "context"),
            },
            Row {
                section: Some("Settings".into()),
                label: "Display".into(),
                badge: None,
                detail: Some("Zen mode.".into()),
                does: category(SettingsNamespace::Tui, "display"),
            },
        ]
    );
}

// Covers: a category page shows each setting's current value by kind (bool,
// model by its catalog label, choice by its label, integer with its unit),
// when a change applies, and the file's problem with a setting.
// Owner: daemon settings schema → `/config` rows (pure).
#[test]
fn setting_rows_show_values_applies_and_problems() {
    let settings = settings();
    let agent = SettingsNamespace::Agent;
    assert_eq!(
        rows(settings.setting_rows(agent, "agent", &models())),
        vec![
            Row {
                section: Some("Advisor".into()),
                label: "Advisor mode".into(),
                badge: Some("on".into()),
                detail: Some("Advisor mode help. Applies from the next turn.".into()),
                does: setting(agent, "behavior.advisor_mode"),
            },
            Row {
                section: Some("Advisor".into()),
                label: "Advisor model".into(),
                badge: Some("GPT-5.5".into()),
                detail: Some("Advisor model help. Applies from the next turn.".into()),
                does: setting(agent, "internal_agents.advisor"),
            },
            Row {
                section: Some("Advisor".into()),
                label: "Inline shell".into(),
                badge: Some("Bash".into()),
                detail: Some("Inline shell help.".into()),
                does: setting(agent, "behavior.inline_shell"),
            },
        ]
    );
    assert_eq!(
        rows(settings.setting_rows(agent, "context", &models())),
        vec![Row {
            section: None,
            label: "Compact threshold".into(),
            badge: Some("80%".into()),
            detail: Some(
                "Compact threshold help. Applies from the next turn.\n\nThe file's value is not used: must be at most 100, not 140"
                    .into()
            ),
            does: setting(agent, "compaction.compact_threshold_percent"),
        }]
    );
}

// Covers: a model setting's page offers the session's models and Claude Code's
// rows as its runtimes allow, marks the current one, and offers a reset once the
// file sets it; a choice left at its default offers no reset.
// Owner: daemon settings schema → `/config` rows (pure).
#[test]
fn value_rows_offer_each_value_and_a_reset_once_set() {
    let settings = settings();
    let agent = SettingsNamespace::Agent;
    let set = |key: &str, value| {
        Some(SettingsRow::Set {
            namespace: agent,
            key: SettingKey::new(key),
            value,
        })
    };
    let model_key = SettingKey::new("internal_agents.advisor");
    assert_eq!(
        rows(settings.value_rows(agent, &model_key, &models()).unwrap())
            .into_iter()
            // Each Claude Code alias gets a row too; the default stands in.
            .filter(
                |row| !row.label.starts_with("claude-code/") || row.label == "claude-code/default"
            )
            .collect::<Vec<_>>(),
        vec![
            Row {
                section: None,
                label: "GPT-5.5".into(),
                badge: Some("current".into()),
                detail: Some("openai/gpt-5.5".into()),
                does: set(
                    "internal_agents.advisor",
                    SettingValue::Model(ModelSelection::Rho {
                        provider: "openai".into(),
                        model: "gpt-5.5".into(),
                    }),
                ),
            },
            Row {
                section: None,
                label: "claude-code/default".into(),
                badge: None,
                detail: Some("Set no model and use whichever model Claude Code selects.".into()),
                does: set(
                    "internal_agents.advisor",
                    SettingValue::Model(ModelSelection::ClaudeCli { model: None }),
                ),
            },
            Row {
                section: None,
                label: "Default (not set)".into(),
                badge: None,
                detail: Some("Remove the setting from the file, so its default applies.".into()),
                does: Some(SettingsRow::Reset {
                    namespace: agent,
                    key: model_key.clone(),
                }),
            },
        ]
    );
    let shell = SettingKey::new("behavior.inline_shell");
    assert_eq!(
        rows(settings.value_rows(agent, &shell, &models()).unwrap())
            .into_iter()
            .map(|row| (row.label, row.badge))
            .collect::<Vec<_>>(),
        vec![
            ("Bash".to_owned(), Some("current".to_owned())),
            ("Zsh".to_owned(), None),
        ]
    );
    let mode = SettingKey::new("behavior.advisor_mode");
    assert!(settings.value_rows(agent, &mode, &models()).is_none());
}

// Covers: a row's value survives encoding for every kind it carries,
// including a model value whose JSON holds `:`; a row that fails to parse
// would do nothing when chosen.
// Owner: daemon settings row values (pure).
#[test]
fn row_values_round_trip() {
    let key = SettingKey::new("internal_agents.advisor");
    let cases = vec![
        SettingsRow::Category {
            namespace: SettingsNamespace::Tui,
            id: "display".into(),
        },
        SettingsRow::Setting {
            namespace: SettingsNamespace::Agent,
            key: key.clone(),
        },
        SettingsRow::Set {
            namespace: SettingsNamespace::Agent,
            key: key.clone(),
            value: SettingValue::Model(ModelSelection::Rho {
                provider: "host:8080".into(),
                model: "a=b".into(),
            }),
        },
        SettingsRow::Set {
            namespace: SettingsNamespace::Tui,
            key: SettingKey::new("zen_mode"),
            value: SettingValue::Bool(true),
        },
        SettingsRow::Reset {
            namespace: SettingsNamespace::Agent,
            key,
        },
    ];
    for row in cases {
        assert_eq!(SettingsRow::parse(&row.encode()), Some(row));
    }
}

// Covers: a change notification carries only changed entries, so the cache
// merges them; its problems show once when they change, not on every
// change after; another client's namespace is ignored.
// Owner: daemon settings cache (pure).
#[test]
fn changes_merge_and_report_new_problems_once() {
    let mut settings = settings();
    let problem = ConfigError {
        key: Some(SettingKey::new("zen_mode")),
        message: "expected a boolean".into(),
    };
    let changed = |revision: &str, value: bool, errors: Vec<ConfigError>| ConfigChangedParams {
        namespace: SettingsNamespace::Tui.wire(),
        revision: ConfigRevision::new(revision),
        values: BTreeMap::from([(
            SettingKey::new("zen_mode"),
            entry(Some(SettingValue::Bool(value)), ValueSource::Global),
        )]),
        errors,
    };

    let first = settings.apply(changed("t2", true, vec![problem.clone()]));
    let second = settings.apply(changed("t3", false, vec![problem.clone()]));
    let fixed = settings.apply(changed("t4", true, Vec::new()));
    let other = settings.apply(ConfigChangedParams {
        namespace: ConfigNamespace::Client {
            name: rho_protocol::ClientName::new("other").unwrap(),
        },
        ..changed("o1", false, Vec::new())
    });

    assert_eq!(
        (first, second, fixed, other),
        (
            Some(Applied {
                namespace: SettingsNamespace::Tui,
                new_problems: vec![problem],
            }),
            Some(Applied {
                namespace: SettingsNamespace::Tui,
                new_problems: Vec::new(),
            }),
            Some(Applied {
                namespace: SettingsNamespace::Tui,
                new_problems: Vec::new(),
            }),
            None,
        )
    );
    assert_eq!(
        settings.values(SettingsNamespace::Tui),
        &values(
            "t4",
            vec![(
                "zen_mode",
                entry(Some(SettingValue::Bool(true)), ValueSource::Global)
            )],
            Vec::new(),
        )
    );
}
