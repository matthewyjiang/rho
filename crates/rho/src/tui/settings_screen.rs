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
//! (`DaemonSession::set_config`), which names the TUI as its writer, so a
//! write sent before `rho/config/changed` for the last one arrives is not
//! refused for it; nothing changes here until that notification says so. Then an open page re-renders with its
//! selection kept, and a `tui` client change takes effect live.

use rho_protocol::{
    Applies, ConfigLayer, ConfigSetParams, SettingChange, SettingKey, SettingKind, SettingValue,
};

use super::{
    config_row::ConfigRow,
    daemon_settings::{display_value, DaemonSettings, SettingsNamespace},
    text_input::TextInput,
    App, ComposerMode, Entry, PickerItem, TuiSettings, UiPicker,
};

/// A settings page, rebuilt from the cached settings whenever they change,
/// so returning to it shows what a child page saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ConfigPage {
    Root,
    SessionModel,
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
        let mut picker = match page {
            ConfigPage::Root => self.daemon_root_picker()?,
            ConfigPage::SessionModel => self.daemon_model_picker()?,
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
        match page {
            // A page of values opens on the one in force, so Enter keeps it.
            // The session's pages do it themselves, as `/model` and
            // `/permissions` open them too.
            ConfigPage::Setting { .. } | ConfigPage::AdvisorOn => picker.select_badged(),
            ConfigPage::Root
            | ConfigPage::Category { .. }
            | ConfigPage::SessionModel
            | ConfigPage::SessionPermissionMode => {}
        }
        Some(picker.with_config_page(page.clone()))
    }

    fn daemon_root_picker(&self) -> Option<UiPicker> {
        let options = self.daemon_options.as_ref()?;
        let session = |label: &str, row: ConfigRow| PickerItem {
            section: Some("Session".into()),
            ..PickerItem::new(label.into(), row.encode())
        };
        let mut items = vec![session("Model", ConfigRow::SessionModel)];
        if options.reasoning.is_some() {
            items.push(session("Reasoning", ConfigRow::SessionReasoning));
        }
        if options.fast_mode.is_some() {
            items.push(session("Fast mode", ConfigRow::SessionFastMode));
        }
        items.push(session("Permission mode", ConfigRow::SessionPermissionMode));
        if let Some(settings) = &self.daemon_settings {
            items.extend(settings.category_rows());
        }
        let local = |label: &str, row: ConfigRow, detail: &str| PickerItem {
            section: Some("This terminal".into()),
            detail: Some(super::picker::PickerDetail::Text(detail.into())),
            ..PickerItem::new(label.into(), row.encode())
        };
        items.push(local(
            "Theme",
            ConfigRow::Theme,
            "The color theme, saved in tui.toml.",
        ));
        items.push(local(
            "Clear prompt history",
            ConfigRow::ClearPromptHistory,
            "Delete every saved composer prompt, after a confirmation.",
        ));
        Some(UiPicker::config("Config · saves automatically", items))
    }

    pub(super) fn daemon_models(&self) -> &[rho_protocol::ModelChoice] {
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
        let Some(row) = ConfigRow::parse(value) else {
            anyhow::bail!("unknown settings row");
        };
        match row {
            // Child pages stack on the page they open from, so Esc returns
            // to it.
            ConfigRow::SessionModel => self.open_config_page(ConfigPage::SessionModel),
            ConfigRow::SessionPermissionMode => {
                self.open_config_page(ConfigPage::SessionPermissionMode);
            }
            ConfigRow::Category { namespace, id } => {
                self.open_config_page(ConfigPage::Category { namespace, id });
            }
            ConfigRow::SessionReasoning => return self.cycle_daemon_reasoning(),
            ConfigRow::SessionFastMode => return self.set_daemon_fast(""),
            ConfigRow::SetPermissionMode { mode } => {
                self.close_value_page();
                self.set_daemon_permission(mode)?;
            }
            ConfigRow::Theme => return self.open_theme_picker_from_config(),
            ConfigRow::ClearPromptHistory => return self.prompt_clear_prompt_history(),
            ConfigRow::Setting { namespace, key } => self.change_setting(namespace, key),
            ConfigRow::Set {
                namespace,
                key,
                value,
            } => {
                // Closed first, so the write's status is the one left.
                self.close_value_page();
                self.write_setting(namespace, SettingChange::Set { key, value });
            }
            ConfigRow::Reset { namespace, key } => {
                self.close_value_page();
                self.write_setting(namespace, SettingChange::Reset { key });
            }
            ConfigRow::EnableAdvisor { model } => {
                self.close_value_page();
                self.enable_advisor(model);
            }
        }
        Ok(())
    }

    /// Opens `page` over the open one.
    fn open_config_page(&mut self, page: ConfigPage) {
        if let Some(picker) = self.config_page_picker(&page) {
            self.open_child_picker(picker);
        }
    }

    /// Leaves a value page once a value is chosen: back to the page it
    /// opened from, or to the composer when it opened alone.
    pub(super) fn close_value_page(&mut self) {
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
                self.open_config_page(ConfigPage::Setting { namespace, key });
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
            Ok(SettingChange::Set {
                key,
                value: SettingValue::Integer(limit),
            }) if lowers_saved_prompts(
                namespace,
                &key,
                limit,
                self.info.services.settings.prompt_history_limit,
            ) =>
            {
                self.close_setting_input();
                if let Err(error) = self.confirm_prompt_history_limit(limit) {
                    self.insert_entry(&Entry::Error(error.to_string()));
                }
            }
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
    pub(super) fn write_setting(&mut self, namespace: SettingsNamespace, change: SettingChange) {
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
    pub(super) fn send_settings(
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
            // `DaemonRequests` names the writer.
            writer: None,
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
        // The `tui` values to apply: all of them once loaded, then only
        // what each change names.
        let (tui_values, problems) = match update {
            SettingsUpdate::Loaded(settings) => {
                let problems = settings.problems();
                let values = settings.values(SettingsNamespace::Tui).values.clone();
                self.daemon_settings = Some(*settings);
                (Some(values), problems)
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
                    (applied.namespace == SettingsNamespace::Tui).then_some(applied.changed),
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
        if let Some(values) = tui_values {
            let next = self.info.services.settings.with_client_values(&values);
            self.apply_tui_settings(next);
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
}

/// Whether setting the prompt history limit to `limit` deletes saved
/// prompts: it lowers a limit that keeps some. `0` stops saving and
/// deletes nothing.
fn lowers_saved_prompts(
    namespace: SettingsNamespace,
    key: &SettingKey,
    limit: u64,
    current: usize,
) -> bool {
    namespace == SettingsNamespace::Tui
        && key.as_str() == super::client_settings::PROMPT_HISTORY_LIMIT_KEY
        && limit > 0
        && limit < current as u64
}

/// `label` as status text names it: lowercase, unless its first word is a
/// name such as `OpenAI` or `RTK`.
pub(super) fn status_label(label: &str) -> String {
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
