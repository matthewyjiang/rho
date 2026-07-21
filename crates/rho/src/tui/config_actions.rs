use rho_providers::credentials::load_web_search_api_key;

use super::{
    config_editor, config_picker, resolve_web_search_editor_value, App, ComposerMode,
    ConfigMutation, ConfigNumberInput, ConfigNumberKey, ConfigTextInput, ConfigTextKey,
    ConfigToggle, Entry, InteractiveRuntime, PickerAction,
};

impl App {
    pub(super) async fn submit_config_selection(
        &mut self,
        value: &str,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match value {
            value if config_picker::is_category(value) => self.open_config_category(value),
            config_picker::CONVERSATION_MODEL_VALUE => {
                self.open_config_conversation_model_picker();
                Ok(())
            }
            config_picker::TITLE_MODEL_VALUE => {
                self.open_config_title_model_picker();
                Ok(())
            }
            config_picker::REFRESH_MODEL_LIST_VALUE => {
                self.open_config_refresh_model_picker();
                Ok(())
            }
            config_picker::PROVIDER_LOGIN_VALUE => {
                self.open_config_login_picker();
                Ok(())
            }
            config_picker::PROVIDER_LOGOUT_VALUE => {
                self.open_config_logout_picker();
                Ok(())
            }
            config_picker::PERMISSION_MODE_VALUE => {
                let child =
                    config_picker::permission_mode_picker(self.info.runtime.permission_mode);
                self.open_child_picker(child);
                Ok(())
            }
            value if value.starts_with(config_picker::PERMISSION_MODE_PREFIX) => {
                let mode = value[config_picker::PERMISSION_MODE_PREFIX.len()..].parse()?;
                self.apply_permission_mode(mode, agent).await?;
                self.open_main_config_picker_selected(config_picker::PERMISSION_MODE_VALUE)
            }
            config_picker::REASONING_VALUE => self.cycle_reasoning(agent),
            config_picker::SHOW_REASONING_OUTPUT_VALUE => self.toggle_reasoning_output(),
            config_picker::CHECK_FOR_UPDATES_VALUE => self.toggle_check_for_updates(),
            config_picker::ENABLE_SUBAGENTS_VALUE => self.toggle_enable_subagents(),
            config_picker::AUTO_COMPACT_VALUE => self.toggle_auto_compact(),
            config_picker::COMPACT_THRESHOLD_PERCENT_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::CompactThresholdPercent,
                    config.compact_threshold_percent as usize,
                ));
                self.status = "edit compact threshold percent".into();
                Ok(())
            }
            config_picker::COMPACT_TARGET_PERCENT_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::CompactTargetPercent,
                    config.compact_target_percent as usize,
                ));
                self.status = "edit compact target percent".into();
                Ok(())
            }
            config_picker::MAX_OUTPUT_BYTES_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxOutputBytes,
                    config.max_output_bytes,
                ));
                self.status = "edit max output bytes".into();
                Ok(())
            }
            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.composer = ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                    ConfigNumberKey::MaxToolOutputLines,
                    config.max_tool_output_lines,
                ));
                self.status = "edit max tool output lines".into();
                Ok(())
            }
            config_picker::INLINE_SHELL_VALUE => {
                let config = self.info.services.config_repository.load()?;
                let child = config_picker::inline_shell_picker(&config);
                self.open_child_picker(child);
                Ok(())
            }
            value if value.starts_with(config_picker::INLINE_SHELL_PREFIX) => {
                let shell = value[config_picker::INLINE_SHELL_PREFIX.len()..].to_string();
                self.info.services.config_repository.update(|config| {
                    config.inline_shell.clone_from(&shell);
                })?;
                self.open_main_config_picker_selected(config_picker::INLINE_SHELL_VALUE)?;
                self.status = format!("inline shell: {shell}");
                Ok(())
            }
            config_picker::WEB_SEARCH_VALUE => {
                let config = self.info.services.config_repository.load()?;
                let child = config_picker::web_search_config_picker(
                    &config,
                    self.credential_store.as_ref(),
                );
                self.open_child_picker(child);
                Ok(())
            }
            config_picker::WEB_SEARCH_PROVIDER_VALUE => self.cycle_web_search_provider(),
            config_picker::WEB_SEARCH_OPENAI_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::OpenAiSearch)
            }
            config_picker::WEB_SEARCH_EXA_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::Exa)
            }
            config_picker::WEB_SEARCH_BRAVE_KEY_VALUE => {
                self.open_web_search_api_key_editor(ConfigTextKey::Brave)
            }
            _ => Ok(()),
        }
    }

    pub(super) fn open_web_search_api_key_editor(
        &mut self,
        key: ConfigTextKey,
    ) -> anyhow::Result<()> {
        let credential = key.web_search_credential();
        let config = self.info.services.config_repository.load()?;
        let (value, load_error) = resolve_web_search_editor_value(
            load_web_search_api_key(self.credential_store.as_ref(), credential),
            config.legacy_web_search_api_key(credential),
        );
        if let Some(err) = load_error {
            self.insert_entry(&Entry::Error(format!(
                "could not access {}: {err}",
                key.label()
            )));
        }
        let return_picker = match std::mem::replace(&mut self.composer, ComposerMode::Input) {
            ComposerMode::Picker(picker) => Some(picker),
            composer => {
                self.composer = composer;
                None
            }
        };
        let mut input = ConfigTextInput::new(key, value);
        if let Some(picker) = return_picker {
            input = input.with_return_picker(picker);
        }
        self.composer = ComposerMode::ConfigTextInput(input);
        self.status = format!("edit {}", key.label());
        Ok(())
    }

    pub(super) fn refresh_main_config_picker(
        &mut self,
        selected_value: &str,
    ) -> anyhow::Result<()> {
        let filter = match &self.composer {
            ComposerMode::Picker(picker) => picker.filter.clone(),
            _ => String::new(),
        };
        self.open_main_config_picker(selected_value, filter)
    }

    pub(super) fn open_main_config_picker_selected(
        &mut self,
        selected_value: &str,
    ) -> anyhow::Result<()> {
        self.open_main_config_picker(selected_value, String::new())
    }

    pub(super) fn open_main_config_picker(
        &mut self,
        selected_value: &str,
        filter: String,
    ) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        let mut root = config_picker::config_picker(&self.info.runtime, &config);
        let Some(category) = config_picker::category_for_setting(selected_value) else {
            Self::restore_picker_position(&mut root, selected_value, filter);
            self.composer = ComposerMode::Picker(root);
            self.status = "config".into();
            return Ok(());
        };

        Self::restore_picker_position(&mut root, category, String::new());
        let mut picker = config_picker::category_picker(category, &self.info.runtime, &config)
            .expect("known config category must have a picker")
            .with_parent(root);
        Self::restore_picker_position(&mut picker, selected_value, filter);
        self.status = picker.title.clone();
        self.composer = ComposerMode::Picker(picker);
        Ok(())
    }

    pub(super) fn open_config_category(&mut self, category: &str) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        let Some(picker) = config_picker::category_picker(category, &self.info.runtime, &config)
        else {
            return Ok(());
        };
        self.open_child_picker(picker);
        Ok(())
    }

    pub(super) fn refresh_web_search_config_picker(
        &mut self,
        selected_value: &str,
    ) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        let (filter, parent) = match &mut self.composer {
            ComposerMode::Picker(picker) => (picker.filter.clone(), picker.take_parent()),
            ComposerMode::ConfigTextInput(input) => match input.take_return_picker() {
                Some(mut picker) => (picker.filter.clone(), picker.take_parent()),
                None => (String::new(), None),
            },
            _ => (String::new(), None),
        };
        let mut picker =
            config_picker::web_search_config_picker(&config, self.credential_store.as_ref());
        Self::restore_picker_position(&mut picker, selected_value, filter);
        if let Some(parent) = parent {
            picker = picker.with_parent(parent);
        }
        self.composer = ComposerMode::Picker(picker);
        Ok(())
    }

    pub(super) fn toggle_check_for_updates(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(
            &self.info.services.config_repository,
            ConfigToggle::CheckForUpdates,
        ) {
            Ok(ConfigMutation::CheckForUpdates(check_for_updates)) => {
                self.info
                    .services
                    .diagnostics
                    .update_check_for_updates(check_for_updates);
                if !check_for_updates {
                    self.info.services.update_notice = None;
                }
                self.status = if check_for_updates {
                    "check for updates: on".into()
                } else {
                    "check for updates: off".into()
                };
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save update check setting: {err}"
                )));
                self.status = "config save failed".into();
            }
            Ok(
                ConfigMutation::EnableSubagents(_)
                | ConfigMutation::AutoCompact(_)
                | ConfigMutation::ShowReasoningOutput(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            self.refresh_main_config_picker(config_picker::CHECK_FOR_UPDATES_VALUE)?;
        }
        Ok(())
    }

    pub(super) fn toggle_enable_subagents(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(
            &self.info.services.config_repository,
            ConfigToggle::EnableSubagents,
        ) {
            Ok(ConfigMutation::EnableSubagents(enable_subagents)) => {
                self.status = if enable_subagents {
                    "subagents: on next session".into()
                } else {
                    "subagents: off next session".into()
                };
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save subagent setting: {err}"
                )));
                self.status = "config save failed".into();
            }
            Ok(
                ConfigMutation::CheckForUpdates(_)
                | ConfigMutation::AutoCompact(_)
                | ConfigMutation::ShowReasoningOutput(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            self.refresh_main_config_picker(config_picker::ENABLE_SUBAGENTS_VALUE)?;
        }
        Ok(())
    }

    pub(super) fn toggle_auto_compact(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(
            &self.info.services.config_repository,
            ConfigToggle::AutoCompact,
        ) {
            Ok(ConfigMutation::AutoCompact(auto_compact)) => {
                self.status = if auto_compact {
                    "auto compact: on".into()
                } else {
                    "auto compact: off".into()
                };
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save auto compact setting: {err}"
                )));
                self.status = "config save failed".into();
            }
            Ok(
                ConfigMutation::CheckForUpdates(_)
                | ConfigMutation::EnableSubagents(_)
                | ConfigMutation::ShowReasoningOutput(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            self.refresh_main_config_picker(config_picker::AUTO_COMPACT_VALUE)?;
        }
        Ok(())
    }

    pub(super) fn toggle_reasoning_output(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(
            &self.info.services.config_repository,
            ConfigToggle::ShowReasoningOutput,
        ) {
            Ok(ConfigMutation::ShowReasoningOutput(show_reasoning_output)) => {
                self.info.runtime.show_reasoning_output = show_reasoning_output;
                self.status = if show_reasoning_output {
                    "reasoning output: shown".into()
                } else {
                    "reasoning output: hidden".into()
                };
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save reasoning output setting: {err}"
                )));
                self.status = "config save failed".into();
            }
            Ok(
                ConfigMutation::CheckForUpdates(_)
                | ConfigMutation::EnableSubagents(_)
                | ConfigMutation::AutoCompact(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        if matches!(
            &self.composer,
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            let config = self
                .info
                .services
                .config_repository
                .load()
                .unwrap_or_default();
            self.info.runtime.show_reasoning_output = config.show_reasoning_output;
            self.refresh_main_config_picker(config_picker::SHOW_REASONING_OUTPUT_VALUE)?;
        }
        Ok(())
    }

    pub(super) fn cycle_web_search_provider(&mut self) -> anyhow::Result<()> {
        let ConfigMutation::WebSearchProvider(provider) =
            config_editor::cycle_web_search_provider(&self.info.services.config_repository)?
        else {
            unreachable!("provider cycle returned a mismatched config mutation");
        };
        self.refresh_web_search_config_picker(config_picker::WEB_SEARCH_PROVIDER_VALUE)?;
        self.status = format!("web search: {provider}");
        Ok(())
    }

    pub(super) fn save_current_config(&self) -> anyhow::Result<()> {
        self.info.services.config_repository.update(|config| {
            config.provider = self.info.runtime.provider.clone();
            config.model = self.info.runtime.model.clone();
            config.auth = self.info.runtime.auth.clone();
            config.reasoning = self.info.runtime.reasoning;
        })
    }
}
