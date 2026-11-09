//! `/config` and `/advisor` in a daemon session (#1720).
//!
//! The root page holds the attached session's own options (model,
//! reasoning, fast mode, permission mode) and the local theme, then one row
//! per settings category: the agent's, from the daemon's schema, and the
//! `tui` client's. A category page lists its settings with their values. A
//! bool flips in place, a choice or model opens a page of its values, and
//! an integer or text opens the line editor, where an empty entry resets it.
//!
//! Every write goes to the daemon with the revision the TUI last saw
//! (`DaemonSession::set_config`); nothing changes here until the daemon's
//! `rho/config/changed` says so. Then an open page re-renders with its
//! selection kept, and a `tui` client change takes effect live.

use rho_protocol::{
    Applies, ConfigLayer, ConfigSetParams, SettingChange, SettingKey, SettingKind, SettingValue,
};

use super::{
    daemon_settings::{display_value, DaemonSettings, SettingsNamespace, SettingsRow},
    text_input::TextInput,
    App, ComposerMode, Entry, PickerItem, TuiSettings, UiPicker,
};

/// A settings page, rebuilt from the cached settings whenever they change,
/// so returning to it shows what a child page saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ConfigPage {
    Root,
    SessionPermissionMode,
    Category {
        namespace: SettingsNamespace,
        id: String,
    },
    Setting {
        namespace: SettingsNamespace,
        key: SettingKey,
    },
    /// The advisor model page `/advisor on` opens: choosing a model also
    /// turns the advisor on.
    AdvisorOn,
}

const FAST_MODE: &str = "daemon_fast_mode";
const ADVISOR_MODE_KEY: &str = "behavior.advisor_mode";
const ADVISOR_MODEL_KEY: &str = "internal_agents.advisor";
/// A row of the `/advisor on` model page: turn the advisor on with this
/// model, in one write.
const ENABLE_ADVISOR_PREFIX: &str = "enable_advisor=";

/// What a settings view changed, as `daemon_transcript` hands it over.
pub(super) enum SettingsUpdate {
    Loaded(Box<DaemonSettings>),
    Changed(Box<rho_protocol::ConfigChangedParams>),
}

impl App {
    pub(super) fn open_daemon_config(&mut self) {
        if let Some(picker) = self.config_page_picker(&ConfigPage::Root) {
            self.input_ui.set_composer(ComposerMode::Picker(picker));
        }
    }

    /// Builds `page` from the session's options and the cached settings;
    /// `None` until the session's options arrived, or once its setting is
    /// gone.
    pub(super) fn config_page_picker(&self, page: &ConfigPage) -> Option<UiPicker> {
        let picker = match page {
            ConfigPage::Root => self.daemon_root_picker()?,
            ConfigPage::SessionPermissionMode => self.daemon_permission_picker()?,
            ConfigPage::Category { namespace, id } => {
                let settings = self.daemon_settings.as_ref()?;
                UiPicker::config(
                    settings.category_label(*namespace, id),
                    settings.setting_rows(*namespace, id, self.daemon_models()),
                )
            }
            ConfigPage::Setting { namespace, key } => {
                let settings = self.daemon_settings.as_ref()?;
                let label = settings.spec(*namespace, key)?.label.clone();
                UiPicker::config(
                    label,
                    settings.value_rows(*namespace, key, self.daemon_models())?,
                )
            }
            ConfigPage::AdvisorOn => self.advisor_on_picker()?,
        };
        Some(picker.with_config_page(page.clone()))
    }

    fn daemon_root_picker(&self) -> Option<UiPicker> {
        let options = self.daemon_options.as_ref()?;
        let session = |label: &str, value: &str| PickerItem {
            section: Some("Session".into()),
            ..PickerItem::new(label.into(), value.into())
        };
        let mut items = vec![session(
            "Model",
            super::config_picker::CONVERSATION_MODEL_VALUE,
        )];
        if options.reasoning.is_some() {
            items.push(session("Reasoning", super::config_picker::REASONING_VALUE));
        }
        if options.fast_mode.is_some() {
            items.push(session("Fast mode", FAST_MODE));
        }
        items.push(session(
            "Permission mode",
            super::config_picker::PERMISSION_MODE_VALUE,
        ));
        if let Some(settings) = &self.daemon_settings {
            items.extend(settings.category_rows());
        }
        let local = |label: &str, value: &str, detail: &str| PickerItem {
            section: Some("This terminal".into()),
            detail: Some(super::picker::PickerDetail::Text(detail.into())),
            ..PickerItem::new(label.into(), value.into())
        };
        items.push(local(
            "Theme",
            super::config_picker::THEME_VALUE,
            "The color theme, saved in tui.toml.",
        ));
        items.push(local(
            "Clear prompt history",
            super::config_picker::CLEAR_PROMPT_HISTORY_VALUE,
            "Delete every saved composer prompt, after a confirmation.",
        ));
        Some(UiPicker::config("Config · saves automatically", items))
    }

    fn daemon_models(&self) -> &[rho_protocol::ModelChoice] {
        self.daemon_options
            .as_ref()
            .map_or(&[], |options| options.model.available.as_slice())
    }

    /// Rebuilds a config page so its rows show the current settings.
    /// Filter, selected row, and parent carry over. Pickers without a
    /// [`ConfigPage`], or whose page is gone, return unchanged.
    pub(super) fn refreshed_config_page(&mut self, mut stale: UiPicker) -> UiPicker {
        let Some(mut fresh) = stale
            .config_page()
            .and_then(|page| self.config_page_picker(&page))
        else {
            return stale;
        };
        let selected = stale
            .selected_item()
            .map(|item| item.value.clone())
            .unwrap_or_default();
        Self::restore_picker_position(&mut fresh, &selected, std::mem::take(&mut stale.filter));
        match stale.take_parent() {
            Some(parent) => fresh.with_parent(parent),
            None => fresh,
        }
    }

    /// Opens the root page at row `selected`, as a local action on it
    /// ends; without the session's options, the composer.
    pub(super) fn reopen_config_root(&mut self, selected: &str) {
        match self.config_page_picker(&ConfigPage::Root) {
            Some(mut picker) => {
                Self::restore_picker_position(&mut picker, selected, String::new());
                self.input_ui.set_composer(ComposerMode::Picker(picker));
            }
            None => self.input_ui.set_composer(ComposerMode::Input),
        }
    }

    /// Re-renders the open config page, as after a settings change.
    pub(super) fn refresh_open_config_page(&mut self) {
        let open = matches!(self.input_ui.composer(), ComposerMode::Picker(picker)
            if picker.config_page().is_some());
        if !open {
            return;
        }
        let ComposerMode::Picker(stale) = self.input_ui.take_composer() else {
            unreachable!("checked above");
        };
        let fresh = self.refreshed_config_page(stale);
        self.input_ui.set_composer(ComposerMode::Picker(fresh));
    }

    /// Runs a row of a config page.
    pub(super) fn commit_daemon_config(&mut self, value: &str) -> anyhow::Result<()> {
        use super::config_picker::{
            CONVERSATION_MODEL_VALUE, PERMISSION_MODE_PREFIX, PERMISSION_MODE_VALUE,
            REASONING_VALUE, THEME_VALUE,
        };
        if let Some(row) = SettingsRow::parse(value) {
            return self.commit_settings_row(row);
        }
        if let Some(model) = value.strip_prefix(ENABLE_ADVISOR_PREFIX) {
            let model: SettingValue = serde_json::from_str(model)?;
            self.enable_advisor(model);
            self.close_value_page();
            return Ok(());
        }
        match value {
            // Child pages stack on the root page, so Esc returns to it.
            CONVERSATION_MODEL_VALUE => {
                if let Some(picker) = self.daemon_model_picker() {
                    self.open_child_picker(picker);
                }
            }
            REASONING_VALUE => return self.cycle_daemon_reasoning(),
            FAST_MODE => return self.set_daemon_fast(""),
            PERMISSION_MODE_VALUE => {
                if let Some(picker) = self.daemon_permission_picker() {
                    self.open_child_picker(picker);
                }
            }
            THEME_VALUE => return self.open_theme_picker_from_config(),
            super::config_picker::CLEAR_PROMPT_HISTORY_VALUE => {
                return self.prompt_clear_prompt_history()
            }
            _ => {
                if let Some(mode) = value.strip_prefix(PERMISSION_MODE_PREFIX) {
                    self.set_daemon_permission(mode.parse()?)?;
                    if !self.pop_picker_level() {
                        self.input_ui.set_composer(ComposerMode::Input);
                    }
                }
            }
        }
        Ok(())
    }

    fn commit_settings_row(&mut self, row: SettingsRow) -> anyhow::Result<()> {
        match row {
            SettingsRow::Category { namespace, id } => {
                if let Some(picker) =
                    self.config_page_picker(&ConfigPage::Category { namespace, id })
                {
                    self.open_child_picker(picker);
                }
            }
            SettingsRow::Setting { namespace, key } => self.change_setting(namespace, key),
            SettingsRow::Set {
                namespace,
                key,
                value,
            } => {
                self.write_setting(namespace, SettingChange::Set { key, value });
                self.close_value_page();
            }
            SettingsRow::Reset { namespace, key } => {
                self.write_setting(namespace, SettingChange::Reset { key });
                self.close_value_page();
            }
        }
        Ok(())
    }

    /// Leaves a value page once a value is chosen: back to the page it
    /// opened from, or to the composer when it opened alone.
    fn close_value_page(&mut self) {
        if !self.pop_picker_level() {
            self.input_ui.set_composer(ComposerMode::Input);
        }
    }

    /// Changes `key` as its kind does: a bool flips, a choice or model
    /// opens its values, an integer or text opens the line editor.
    fn change_setting(&mut self, namespace: SettingsNamespace, key: SettingKey) {
        let Some(settings) = &self.daemon_settings else {
            return;
        };
        let Some(spec) = settings.spec(namespace, &key) else {
            return;
        };
        let current = settings.value(namespace, &key).cloned();
        match &spec.kind {
            SettingKind::Bool => {
                let on = matches!(
                    current.or_else(|| spec.default.clone()),
                    Some(SettingValue::Bool(true))
                );
                self.write_setting(
                    namespace,
                    SettingChange::Set {
                        key,
                        value: SettingValue::Bool(!on),
                    },
                );
            }
            SettingKind::Choice { .. } | SettingKind::Model { .. } => {
                if let Some(picker) =
                    self.config_page_picker(&ConfigPage::Setting { namespace, key })
                {
                    self.open_child_picker(picker);
                }
            }
            SettingKind::Integer { min, max, unit } => {
                let range = match max {
                    Some(max) => format!("{min}–{max}"),
                    None => format!("at least {min}"),
                };
                let unit = match unit {
                    Some(rho_protocol::IntegerUnit::Percent) => " %",
                    Some(rho_protocol::IntegerUnit::Seconds) => " seconds",
                    Some(rho_protocol::IntegerUnit::Lines) => " lines",
                    Some(rho_protocol::IntegerUnit::Bytes) => " bytes",
                    _ => "",
                };
                let label = format!("{} ({range}{unit})", status_label(&spec.label));
                let value = match current {
                    Some(SettingValue::Integer(number)) => number.to_string(),
                    _ => String::new(),
                };
                self.open_setting_input(TextInput::setting(namespace, key, label, value));
            }
            SettingKind::Text => {
                let label = status_label(&spec.label);
                let value = match current {
                    Some(SettingValue::Text(text)) => text,
                    _ => String::new(),
                };
                self.open_setting_input(TextInput::setting(namespace, key, label, value));
            }
            _ => self.set_status(format!(
                "this rho cannot change {}",
                status_label(&spec.label)
            )),
        }
    }

    /// Opens `input` over the open page, which returns once it closes.
    fn open_setting_input(&mut self, input: TextInput) {
        let ComposerMode::Picker(page) = self.input_ui.take_composer() else {
            return;
        };
        self.set_status_quiet(format!("edit {}", input.label()));
        self.input_ui
            .set_composer(ComposerMode::TextInput(input.with_return_picker(page)));
    }

    /// Saves what the line editor holds for `key`; empty resets it.
    pub(super) fn commit_setting_input(
        &mut self,
        namespace: SettingsNamespace,
        key: SettingKey,
        text: &str,
    ) {
        let Some(spec) = self
            .daemon_settings
            .as_ref()
            .and_then(|settings| settings.spec(namespace, &key))
        else {
            return;
        };
        let text = text.trim();
        let change = if text.is_empty() {
            Ok(SettingChange::Reset { key })
        } else {
            match &spec.kind {
                SettingKind::Integer { .. } => text
                    .parse()
                    .map(|number| SettingChange::Set {
                        key,
                        value: SettingValue::Integer(number),
                    })
                    .map_err(|_| "enter a whole number"),
                _ => Ok(SettingChange::Set {
                    key,
                    value: SettingValue::Text(text.to_owned()),
                }),
            }
        };
        match change {
            Ok(change) => {
                self.close_setting_input();
                self.write_setting(namespace, change);
            }
            Err(problem) => self.insert_entry(&Entry::Error(format!(
                "could not save {}: {problem}",
                status_label(&spec.label)
            ))),
        }
    }

    /// Closes the line editor, back to the page it opened over.
    pub(super) fn close_setting_input(&mut self) {
        let page = match self.input_ui.composer_mut() {
            ComposerMode::TextInput(input) => input.take_return_picker(),
            _ => None,
        };
        match page {
            Some(page) => {
                let page = self.refreshed_config_page(page);
                self.set_status_quiet(page.title.clone());
                self.input_ui.set_composer(ComposerMode::Picker(page));
            }
            None => self.input_ui.set_composer(ComposerMode::Input),
        }
    }

    /// Sends `change` to the daemon and says what it will do.
    fn write_setting(&mut self, namespace: SettingsNamespace, change: SettingChange) {
        let key = match &change {
            SettingChange::Set { key, .. } | SettingChange::Reset { key } => key.clone(),
        };
        let Some(spec) = self
            .daemon_settings
            .as_ref()
            .and_then(|settings| settings.spec(namespace, &key))
        else {
            return;
        };
        let label = status_label(&spec.label);
        let shown = match &change {
            SettingChange::Set { value, .. } => {
                display_value(spec, Some(value), self.daemon_models())
            }
            SettingChange::Reset { .. } => "default".into(),
        };
        let status = match applies_status(spec.applies) {
            Some(when) => format!("{label}: {shown}; {when}"),
            None => format!("{label}: {shown}"),
        };
        if self.send_settings(namespace, vec![change], &label) {
            self.set_status(status);
        }
    }

    /// Sends `changes` as one write; `false` once the TUI cannot.
    fn send_settings(
        &mut self,
        namespace: SettingsNamespace,
        changes: Vec<SettingChange>,
        label: &str,
    ) -> bool {
        let (Some(session), Some(settings)) = (&self.info.session.daemon, &self.daemon_settings)
        else {
            return false;
        };
        let params = ConfigSetParams {
            namespace: namespace.wire(),
            layer: ConfigLayer::Global,
            revision: settings.values(namespace).revision.clone(),
            changes,
        };
        match session.set_config(params, label.to_owned()) {
            Ok(()) => true,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not save {label}: {error}")));
                false
            }
        }
    }

    /// Takes in what `daemon_session` read or was told of the settings.
    /// A file's problems show once each time they change; the daemon keeps
    /// its last good values meanwhile.
    pub(super) fn show_daemon_settings(&mut self, update: SettingsUpdate) {
        let (tui_changed, problems) = match update {
            SettingsUpdate::Loaded(settings) => {
                let problems = settings.problems();
                self.daemon_settings = Some(*settings);
                (true, problems)
            }
            SettingsUpdate::Changed(changed) => {
                let Some(applied) = self
                    .daemon_settings
                    .as_mut()
                    .and_then(|settings| settings.apply(*changed))
                else {
                    return;
                };
                (
                    applied.namespace == SettingsNamespace::Tui,
                    applied.new_problems,
                )
            }
        };
        if !problems.is_empty() {
            let problems = problems
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("; ");
            self.insert_entry(&Entry::Error(format!(
                "could not use the edited settings, so the last good ones stay: {problems}"
            )));
        }
        if tui_changed {
            if let Some(settings) = &self.daemon_settings {
                let values = &settings.values(SettingsNamespace::Tui).values;
                let next = self.info.services.settings.with_client_values(values);
                self.apply_tui_settings(next);
            }
        }
        self.refresh_open_config_page();
    }

    /// Makes `next` the TUI's settings, with each change's live effect.
    fn apply_tui_settings(&mut self, next: TuiSettings) {
        let TuiSettings {
            zen_mode,
            show_reasoning_output,
            output_streaming,
            show_header_hints,
            notifications,
            cache_miss_notices,
            max_tool_output_lines,
            prompt_history_limit,
            check_for_updates,
            // Local to `tui.toml`; the daemon never serves them.
            theme: _,
            keybindings,
        } = next.clone();
        let runtime = &mut self.info.runtime;
        // Each changes how history renders.
        let rerender = runtime.zen_mode != zen_mode
            || runtime.show_reasoning_output != show_reasoning_output
            || runtime.max_tool_output_lines != max_tool_output_lines;
        runtime.zen_mode = zen_mode;
        runtime.show_reasoning_output = show_reasoning_output;
        runtime.max_tool_output_lines = max_tool_output_lines.max(1);
        runtime.show_header_hints = show_header_hints;
        runtime.notifications = notifications;
        runtime.cache_miss_notices = cache_miss_notices;
        let vim_changed = runtime.keybindings.editing_mode != keybindings.editing_mode;
        runtime.keybindings.editing_mode = keybindings.editing_mode;
        if rerender {
            self.history.invalidate_from(0);
        }
        if vim_changed {
            // Start the draft in insert mode, as a fresh vim session would.
            self.input_ui.reset_vim();
        }
        if !check_for_updates {
            self.info.services.update_notice = None;
        }
        self.apply_streaming_mode(output_streaming);
        let history_limit_changed =
            self.info.services.settings.prompt_history_limit != prompt_history_limit;
        self.info.services.settings = next;
        if history_limit_changed {
            self.apply_prompt_history_limit(prompt_history_limit);
        }
    }

    /// `/advisor [on|off|model]` in a daemon session.
    pub(super) fn execute_daemon_advisor_command(&mut self, args: &str) {
        let Some(settings) = &self.daemon_settings else {
            self.insert_entry(&Entry::Error(
                "could not change the advisor: the rho settings are not loaded".into(),
            ));
            return;
        };
        let advisor_on = matches!(
            settings.value(SettingsNamespace::Agent, &SettingKey::new(ADVISOR_MODE_KEY)),
            Some(SettingValue::Bool(true))
        );
        let has_model = settings
            .value(
                SettingsNamespace::Agent,
                &SettingKey::new(ADVISOR_MODEL_KEY),
            )
            .is_some();
        let requested = match args.trim().to_ascii_lowercase().as_str() {
            "" => !advisor_on,
            "on" => true,
            "off" => false,
            "model" => {
                self.open_advisor_model_page(/*enable*/ false);
                return;
            }
            _ => {
                self.insert_entry(&Entry::Error(super::advisor_command::ADVISOR_USAGE.into()));
                self.set_status("invalid advisor mode");
                return;
            }
        };
        // Advisor mode on with no model does nothing, so it asks for one.
        if requested && !has_model {
            self.open_advisor_model_page(/*enable*/ true);
            return;
        }
        self.write_setting(
            SettingsNamespace::Agent,
            SettingChange::Set {
                key: SettingKey::new(ADVISOR_MODE_KEY),
                value: SettingValue::Bool(requested),
            },
        );
    }

    /// Opens the advisor model page alone. With `enable`, choosing a model
    /// also turns the advisor on, in the same write.
    fn open_advisor_model_page(&mut self, enable: bool) {
        let page = if enable {
            ConfigPage::AdvisorOn
        } else {
            ConfigPage::Setting {
                namespace: SettingsNamespace::Agent,
                key: SettingKey::new(ADVISOR_MODEL_KEY),
            }
        };
        let Some(picker) = self.config_page_picker(&page) else {
            return;
        };
        self.set_status("select the advisor model");
        self.input_ui.set_composer(ComposerMode::Picker(picker));
    }

    /// The advisor model page, each model row turning the advisor on with
    /// it. No reset row: that would turn the advisor on without a model.
    fn advisor_on_picker(&self) -> Option<UiPicker> {
        let settings = self.daemon_settings.as_ref()?;
        let key = SettingKey::new(ADVISOR_MODEL_KEY);
        let label = settings.spec(SettingsNamespace::Agent, &key)?.label.clone();
        let mut items =
            settings.value_rows(SettingsNamespace::Agent, &key, self.daemon_models())?;
        items.retain_mut(|item| {
            let Some(SettingsRow::Set { value, .. }) = SettingsRow::parse(&item.value) else {
                return false;
            };
            item.value = format!(
                "{ENABLE_ADVISOR_PREFIX}{}",
                serde_json::to_string(&value).expect("setting values serialize")
            );
            true
        });
        Some(UiPicker::config(label, items))
    }

    /// Turns the advisor on with `model`, in one write.
    fn enable_advisor(&mut self, model: SettingValue) {
        let changes = vec![
            SettingChange::Set {
                key: SettingKey::new(ADVISOR_MODEL_KEY),
                value: model,
            },
            SettingChange::Set {
                key: SettingKey::new(ADVISOR_MODE_KEY),
                value: SettingValue::Bool(true),
            },
        ];
        if self.send_settings(SettingsNamespace::Agent, changes, "advisor") {
            self.set_status("advisor: on; applies next turn");
        }
    }
}

/// `label` as status text names it: lowercase, unless its first word is a
/// name such as `OpenAI` or `RTK`.
fn status_label(label: &str) -> String {
    let first = label.split_whitespace().next().unwrap_or_default();
    if first.chars().skip(1).any(char::is_uppercase) {
        return label.to_owned();
    }
    let mut chars = label.chars();
    match chars.next() {
        Some(head) => head.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// When a write takes effect, as its status says it.
fn applies_status(applies: Applies) -> Option<&'static str> {
    match applies {
        Applies::NextTurn => Some("applies next turn"),
        Applies::NextSession => Some("applies to new sessions"),
        Applies::Restart => Some("applies once rho restarts"),
        Applies::Immediately | Applies::Unknown => None,
        _ => None,
    }
}
