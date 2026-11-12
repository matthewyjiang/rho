//! `/advisor [on|off|model]`, which changes the daemon's advisor settings
//! (#1720).

use rho_protocol::{ModelSelection, SettingChange, SettingKey, SettingValue};

use super::{
    advisor_status::AdvisorStatus,
    config_row::ConfigRow,
    daemon_settings::{DaemonSettings, SettingsNamespace},
    settings_screen::ConfigPage,
    App, ComposerMode, Entry, UiPicker,
};

const ADVISOR_MODE_KEY: &str = "behavior.advisor_mode";
const ADVISOR_MODEL_KEY: &str = "internal_agents.advisor";

pub(super) const ADVISOR_USAGE: &str = "usage: /advisor [on|off|model]";

impl App {
    /// The advisor as every surface shows it: the daemon's saved settings
    /// once read, which `/advisor` and `/config` change, and the startup
    /// config's until then.
    pub(super) fn advisor_status(&self) -> AdvisorStatus {
        let Some(settings) = &self.daemon_settings else {
            return AdvisorStatus::from_runtime(&self.info.runtime, self.model_names());
        };
        let model = match settings.value(
            SettingsNamespace::Agent,
            &SettingKey::new(ADVISOR_MODEL_KEY),
        ) {
            Some(SettingValue::Model(ModelSelection::Rho { provider, model })) => {
                Some(self.model_names().reference(provider, model))
            }
            Some(SettingValue::Model(ModelSelection::ClaudeCli { model })) => {
                Some(rho_providers::provider::model_reference(
                    crate::claude_runtime::models::CLAUDE_CODE_SOURCE_LABEL,
                    model
                        .as_deref()
                        .unwrap_or(crate::claude_runtime::models::CLAUDE_DEFAULT_MODEL_BADGE),
                ))
            }
            _ => None,
        };
        AdvisorStatus::of(advisor_on(settings), model)
    }

    /// `/advisor [on|off|model]` in a daemon session.
    pub(super) fn execute_daemon_advisor_command(&mut self, args: &str) {
        let Some(settings) = &self.daemon_settings else {
            self.insert_entry(&Entry::Error(
                "could not change the advisor: the rho settings are not loaded".into(),
            ));
            return;
        };
        let advisor_on = advisor_on(settings);
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
                self.insert_entry(&Entry::Error(ADVISOR_USAGE.into()));
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
    pub(super) fn advisor_on_picker(&self) -> Option<UiPicker> {
        let settings = self.daemon_settings.as_ref()?;
        let key = SettingKey::new(ADVISOR_MODEL_KEY);
        let label = settings.spec(SettingsNamespace::Agent, &key)?.label.clone();
        let mut items =
            settings.value_rows(SettingsNamespace::Agent, &key, self.daemon_models())?;
        items.retain_mut(|item| {
            let Some(ConfigRow::Set { value, .. }) = ConfigRow::parse(&item.value) else {
                return false;
            };
            item.value = ConfigRow::EnableAdvisor { model: value }.encode();
            true
        });
        Some(UiPicker::config(label, items))
    }

    /// Whether `change` resets the advisor's model while the advisor is on,
    /// which would leave it on with nothing to consult.
    pub(super) fn resets_the_advisor_model(
        &self,
        namespace: SettingsNamespace,
        change: &SettingChange,
    ) -> bool {
        self.daemon_settings.as_ref().is_some_and(advisor_on)
            && namespace == SettingsNamespace::Agent
            && matches!(change, SettingChange::Reset { key } if key.as_str() == ADVISOR_MODEL_KEY)
    }

    /// Resets the advisor's model and turns the advisor off, in one write.
    pub(super) fn reset_advisor_model(&mut self) {
        let changes = vec![
            SettingChange::Reset {
                key: SettingKey::new(ADVISOR_MODEL_KEY),
            },
            SettingChange::Set {
                key: SettingKey::new(ADVISOR_MODE_KEY),
                value: SettingValue::Bool(false),
            },
        ];
        if self.send_settings(SettingsNamespace::Agent, changes, "advisor") {
            self.set_status("advisor: off, model reset; applies next turn");
        }
    }

    /// Turns the advisor on with `model`, in one write.
    pub(super) fn enable_advisor(&mut self, model: SettingValue) {
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

/// Whether the saved settings turn advisor mode on.
fn advisor_on(settings: &DaemonSettings) -> bool {
    matches!(
        settings.value(SettingsNamespace::Agent, &SettingKey::new(ADVISOR_MODE_KEY)),
        Some(SettingValue::Bool(true))
    )
}
