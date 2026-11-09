//! The daemon's settings as the TUI shows them (#1720).
//!
//! Once it attaches, a followed session registers the `tui` client's
//! settings, then reads the agent schema and both namespaces' values
//! (`daemon_session`); `rho/config/changed` keeps them current.
//! [`DaemonSettings`] is that cache, and maps it to `/config` rows;
//! `settings_screen` owns the pages and what choosing a row does.

use std::collections::BTreeMap;

use rho_protocol::{
    Applies, ClientName, ConfigChangedParams, ConfigError, ConfigGetResponse, ConfigNamespace,
    ConfigRevision, ConfigSchema, IntegerUnit, ModelRuntime, ModelSelection, SettingEntry,
    SettingKey, SettingKind, SettingSpec, SettingValue,
};

use super::{
    config_row::ConfigRow, picker::PickerDetail, PickerBadge, PickerBadgeTone, PickerItem,
};

/// The `tui` client's namespace name.
const TUI_CLIENT: &str = "tui";

/// The agent setting `/config` leaves out: the daemon changes the
/// credential store only through `rho/auth/choose_store`, which checks the
/// store can be reached, so `rho credential-store set` and the login
/// question own it.
const CREDENTIAL_STORE_KEY: &str = "behavior.credential_store";

/// The settings namespaces the TUI reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SettingsNamespace {
    /// `config.toml`, described by the daemon's schema.
    Agent,
    /// `clients/tui.toml`, described by the TUI's own registry.
    Tui,
}

impl SettingsNamespace {
    pub(super) fn wire(self) -> ConfigNamespace {
        match self {
            Self::Agent => ConfigNamespace::Agent,
            Self::Tui => ConfigNamespace::Client { name: tui_client() },
        }
    }

    fn of(namespace: &ConfigNamespace) -> Option<Self> {
        match namespace {
            ConfigNamespace::Agent => Some(Self::Agent),
            ConfigNamespace::Client { name } if name.as_str() == TUI_CLIENT => Some(Self::Tui),
            _ => None,
        }
    }
}

pub(crate) fn tui_client() -> ClientName {
    ClientName::new(TUI_CLIENT).expect("`tui` is a valid client name")
}

/// One namespace's values, as the daemon last sent them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NamespaceValues {
    pub(super) revision: ConfigRevision,
    pub(super) values: BTreeMap<SettingKey, SettingEntry>,
    /// Problems in the file; the daemon keeps its last good values.
    pub(super) errors: Vec<ConfigError>,
}

impl From<ConfigGetResponse> for NamespaceValues {
    fn from(response: ConfigGetResponse) -> Self {
        Self {
            revision: response.revision,
            values: response.values,
            errors: response.errors,
        }
    }
}

/// What one `rho/config/changed` did to the cache.
#[derive(Debug, PartialEq)]
pub(super) struct Applied {
    pub(super) namespace: SettingsNamespace,
    /// Only the entries that changed, so a live setting the TUI changed
    /// for itself, such as Alt+S streaming, survives another's change.
    pub(super) changed: BTreeMap<SettingKey, SettingEntry>,
    /// The namespace's problems, when they changed; empty otherwise, or
    /// once the file is fixed.
    pub(super) new_problems: Vec<ConfigError>,
}

/// What the TUI knows of the daemon's settings.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DaemonSettings {
    schema: ConfigSchema,
    tui_specs: Vec<SettingSpec>,
    agent: NamespaceValues,
    tui: NamespaceValues,
}

impl DaemonSettings {
    pub(crate) fn new(
        mut schema: ConfigSchema,
        tui_specs: Vec<SettingSpec>,
        agent: NamespaceValues,
        tui: NamespaceValues,
    ) -> Self {
        schema
            .settings
            .retain(|spec| spec.key.as_str() != CREDENTIAL_STORE_KEY);
        Self {
            schema,
            tui_specs,
            agent,
            tui,
        }
    }

    /// Takes in a `rho/config/changed`; `None` for a namespace the TUI
    /// never read.
    pub(super) fn apply(&mut self, changed: ConfigChangedParams) -> Option<Applied> {
        let namespace = SettingsNamespace::of(&changed.namespace)?;
        let current = self.values_mut(namespace);
        current.revision = changed.revision;
        current.values.extend(changed.values.clone());
        let new_problems = if current.errors == changed.errors {
            Vec::new()
        } else {
            changed.errors.clone()
        };
        current.errors = changed.errors;
        Some(Applied {
            namespace,
            changed: changed.values,
            new_problems,
        })
    }

    /// Every problem in the files, as when the settings were first read.
    pub(super) fn problems(&self) -> Vec<ConfigError> {
        [&self.agent, &self.tui]
            .into_iter()
            .flat_map(|values| values.errors.iter().cloned())
            .collect()
    }

    pub(super) fn values(&self, namespace: SettingsNamespace) -> &NamespaceValues {
        match namespace {
            SettingsNamespace::Agent => &self.agent,
            SettingsNamespace::Tui => &self.tui,
        }
    }

    fn values_mut(&mut self, namespace: SettingsNamespace) -> &mut NamespaceValues {
        match namespace {
            SettingsNamespace::Agent => &mut self.agent,
            SettingsNamespace::Tui => &mut self.tui,
        }
    }

    fn specs(&self, namespace: SettingsNamespace) -> &[SettingSpec] {
        match namespace {
            SettingsNamespace::Agent => &self.schema.settings,
            SettingsNamespace::Tui => &self.tui_specs,
        }
    }

    pub(super) fn spec(
        &self,
        namespace: SettingsNamespace,
        key: &SettingKey,
    ) -> Option<&SettingSpec> {
        self.specs(namespace).iter().find(|spec| &spec.key == key)
    }

    /// The value `key` has now; `None` for an unset optional setting.
    pub(super) fn value(
        &self,
        namespace: SettingsNamespace,
        key: &SettingKey,
    ) -> Option<&SettingValue> {
        self.values(namespace).values.get(key)?.value.as_ref()
    }

    /// The root page's category rows: the agent's, in schema order, then
    /// the `tui` client's. A category without settings is left out.
    pub(super) fn category_rows(&self) -> Vec<PickerItem> {
        let agent = self.schema.categories.iter().map(|category| {
            (
                SettingsNamespace::Agent,
                category.id.clone(),
                category.label.clone(),
            )
        });
        let mut tui_categories: Vec<String> = Vec::new();
        for spec in &self.tui_specs {
            if !tui_categories.contains(&spec.category) {
                tui_categories.push(spec.category.clone());
            }
        }
        let tui = tui_categories
            .into_iter()
            .map(|id| (SettingsNamespace::Tui, id.clone(), capitalized(&id)));
        agent
            .chain(tui)
            .filter_map(|(namespace, id, label)| {
                let specs: Vec<_> = self
                    .specs(namespace)
                    .iter()
                    .filter(|spec| spec.category == id)
                    .collect();
                if specs.is_empty() {
                    return None;
                }
                let detail = specs
                    .iter()
                    .map(|spec| spec.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(PickerItem {
                    section: Some("Settings".into()),
                    detail: Some(PickerDetail::Text(format!("{detail}."))),
                    ..PickerItem::new(label, ConfigRow::Category { namespace, id }.encode())
                })
            })
            .collect()
    }

    /// The label a category page shows as its title.
    pub(super) fn category_label(&self, namespace: SettingsNamespace, id: &str) -> String {
        match namespace {
            SettingsNamespace::Agent => self
                .schema
                .categories
                .iter()
                .find(|category| category.id == id)
                .map_or_else(|| capitalized(id), |category| category.label.clone()),
            SettingsNamespace::Tui => capitalized(id),
        }
    }

    /// Category `id`'s setting rows, in registry order, under their
    /// sections. A row's badge is its value; its detail is the help, when
    /// the change applies, and the file's problem with it, if any.
    pub(super) fn setting_rows(
        &self,
        namespace: SettingsNamespace,
        id: &str,
        models: &[rho_protocol::ModelChoice],
    ) -> Vec<PickerItem> {
        let current = self.values(namespace);
        self.specs(namespace)
            .iter()
            .filter(|spec| spec.category == id)
            .map(|spec| {
                let value = current
                    .values
                    .get(&spec.key)
                    .and_then(|entry| entry.value.as_ref());
                let mut detail = spec.help.clone();
                if let Some(when) = applies_suffix(spec.applies) {
                    detail.push(' ');
                    detail.push_str(when);
                }
                for error in current
                    .errors
                    .iter()
                    .filter(|error| error.key.as_ref() == Some(&spec.key))
                {
                    detail.push_str(&format!(
                        "\n\nThe file's value is not used: {}",
                        error.message
                    ));
                }
                PickerItem {
                    section: spec.section.clone(),
                    detail: Some(PickerDetail::Text(detail)),
                    badge: Some(selected_badge(display_value(spec, value, models))),
                    selection_verb: Some(match spec.kind {
                        SettingKind::Bool => "toggle",
                        _ => "change",
                    }),
                    space_confirms: matches!(spec.kind, SettingKind::Bool),
                    ..PickerItem::new(
                        spec.label.clone(),
                        ConfigRow::Setting {
                            namespace,
                            key: spec.key.clone(),
                        }
                        .encode(),
                    )
                }
            })
            .collect()
    }

    /// The child page of a choice or model setting: one row per value it
    /// takes, the current one badged, then a reset row once the file sets
    /// it. `None` for other kinds, which change without a page.
    pub(super) fn value_rows(
        &self,
        namespace: SettingsNamespace,
        key: &SettingKey,
        models: &[rho_protocol::ModelChoice],
    ) -> Option<Vec<PickerItem>> {
        let spec = self.spec(namespace, key)?;
        let current = self.value(namespace, key);
        let set = |value: SettingValue| ConfigRow::Set {
            namespace,
            key: key.clone(),
            value,
        };
        let mut rows: Vec<PickerItem> = match &spec.kind {
            SettingKind::Choice { choices } => choices
                .iter()
                .map(|choice| {
                    let value = SettingValue::Text(choice.value.clone());
                    PickerItem {
                        detail: choice.help.clone().map(PickerDetail::Text),
                        badge: (current == Some(&value)).then(|| selected_badge("current")),
                        ..PickerItem::new(choice.label.clone(), set(value).encode())
                    }
                })
                .collect(),
            SettingKind::Model { runtimes } => {
                let mut rows = Vec::new();
                if runtimes.contains(&ModelRuntime::Rho) {
                    rows.extend(models.iter().map(|choice| {
                        let value = SettingValue::Model(ModelSelection::Rho {
                            provider: choice.model.provider.clone(),
                            model: choice.model.model.clone(),
                        });
                        PickerItem {
                            detail: Some(PickerDetail::Text(format!(
                                "{}/{}",
                                choice.model.provider, choice.model.model
                            ))),
                            badge: (current == Some(&value)).then(|| selected_badge("current")),
                            ..PickerItem::new(choice.label.clone(), set(value).encode())
                        }
                    }));
                }
                if runtimes.contains(&ModelRuntime::ClaudeCli) {
                    rows.extend(claude_code_rows(current).into_iter().map(
                        |(label, detail, model)| {
                            let value = SettingValue::Model(ModelSelection::ClaudeCli { model });
                            PickerItem {
                                detail: Some(PickerDetail::Text(detail)),
                                badge: (current == Some(&value)).then(|| selected_badge("current")),
                                ..PickerItem::new(label, set(value).encode())
                            }
                        },
                    ));
                }
                rows
            }
            // Bool, integer, and text change in place; `Unknown` cannot.
            _ => return None,
        };
        let from_file = self
            .values(namespace)
            .values
            .get(key)
            .is_some_and(|entry| entry.source == rho_protocol::ValueSource::Global);
        if from_file {
            let default = display_value(spec, spec.default.as_ref(), models);
            rows.push(PickerItem {
                detail: Some(PickerDetail::Text(
                    "Remove the setting from the file, so its default applies.".into(),
                )),
                ..PickerItem::new(
                    format!("Default ({default})"),
                    ConfigRow::Reset {
                        namespace,
                        key: key.clone(),
                    }
                    .encode(),
                )
            });
        }
        Some(rows)
    }
}

/// Claude Code's model rows as `(label, detail, model)`: its own default,
/// each alias, then `current`'s model when no alias names it.
fn claude_code_rows(current: Option<&SettingValue>) -> Vec<(String, String, Option<String>)> {
    use crate::claude_runtime::models::{
        is_offered_alias, CLAUDE_CODE_SOURCE_LABEL, CLAUDE_DEFAULT_MODEL_BADGE,
        CLAUDE_DEFAULT_MODEL_DETAIL, CLAUDE_MODEL_ALIASES,
    };
    let label = |model: &str| format!("{CLAUDE_CODE_SOURCE_LABEL}/{model}");
    let mut rows = vec![(
        label(CLAUDE_DEFAULT_MODEL_BADGE),
        CLAUDE_DEFAULT_MODEL_DETAIL.to_owned(),
        None,
    )];
    rows.extend(CLAUDE_MODEL_ALIASES.iter().map(|alias| {
        (
            label(alias.name),
            alias.detail.to_owned(),
            Some(alias.name.to_owned()),
        )
    }));
    if let Some(SettingValue::Model(ModelSelection::ClaudeCli { model: Some(model) })) = current {
        if !is_offered_alias(model) {
            rows.push((
                label(model),
                "Runs on Claude Code.".to_owned(),
                Some(model.clone()),
            ));
        }
    }
    rows
}

/// `value` as a row's badge says it.
pub(super) fn display_value(
    spec: &SettingSpec,
    value: Option<&SettingValue>,
    models: &[rho_protocol::ModelChoice],
) -> String {
    let Some(value) = value else {
        return "not set".into();
    };
    match (&spec.kind, value) {
        (SettingKind::Bool, SettingValue::Bool(on)) => if *on { "on" } else { "off" }.into(),
        (SettingKind::Choice { choices }, SettingValue::Text(text)) => choices
            .iter()
            .find(|choice| &choice.value == text)
            .map_or_else(|| text.clone(), |choice| choice.label.clone()),
        (SettingKind::Integer { unit, .. }, SettingValue::Integer(number)) => match unit {
            Some(IntegerUnit::Percent) => format!("{number}%"),
            Some(IntegerUnit::Seconds) => format!("{number}s"),
            Some(IntegerUnit::Lines) => format!("{number} lines"),
            Some(IntegerUnit::Bytes) => format!("{number} bytes"),
            // `Unknown`, or a unit a newer daemon sends.
            _ => number.to_string(),
        },
        (_, SettingValue::Model(ModelSelection::Rho { provider, model })) => models
            .iter()
            .find(|choice| &choice.model.provider == provider && &choice.model.model == model)
            .map_or_else(
                || format!("{provider}/{model}"),
                |choice| choice.label.clone(),
            ),
        (_, SettingValue::Model(ModelSelection::ClaudeCli { model })) => {
            format!("claude-code/{}", model.as_deref().unwrap_or("default"))
        }
        (_, SettingValue::Text(text)) => text.clone(),
        (_, SettingValue::Integer(number)) => number.to_string(),
        (_, SettingValue::Bool(on)) => on.to_string(),
    }
}

/// When a change takes effect, as a row's detail ends.
fn applies_suffix(applies: Applies) -> Option<&'static str> {
    match applies {
        Applies::Immediately => None,
        Applies::NextTurn => Some("Applies from the next turn."),
        Applies::NextSession => Some("Applies to new sessions."),
        Applies::Restart => Some("Applies once rho restarts."),
        _ => None,
    }
}

fn selected_badge(text: impl Into<String>) -> PickerBadge {
    PickerBadge {
        text: text.into(),
        tone: PickerBadgeTone::Selected,
    }
}

fn capitalized(id: &str) -> String {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
#[path = "daemon_settings_tests.rs"]
mod tests;
