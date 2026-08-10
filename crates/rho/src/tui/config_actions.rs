use rho_providers::credentials::load_web_search_api_key;

use super::{
    config_editor, config_picker, resolve_web_search_editor_value, App, ComposerMode,
    ConfigMutation, ConfigNumberInput, ConfigNumberKey, ConfigTextKey, ConfigToggle, Entry,
    InteractiveRuntime, PickerAction,
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
            config_picker::REFRESH_MODEL_LIST_VALUE => {
                self.open_config_refresh_model_picker();
                Ok(())
            }
            config_picker::PROVIDER_LOGIN_VALUE => {
                self.open_config_login_picker();
                Ok(())
            }
            config_picker::PROVIDER_LOGOUT_VALUE => self.open_config_logout_picker().await,
            config_picker::SWITCH_AUTH_MODE_VALUE => self.open_config_auth_mode_picker(),
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
            config_picker::ZEN_MODE_VALUE => self.toggle_zen_mode(),
            config_picker::THEME_VALUE => self.open_theme_picker_from_config(),
            config_picker::CHECK_FOR_UPDATES_VALUE => self.toggle_check_for_updates(),
            config_picker::ENABLE_SUBAGENTS_VALUE => self.toggle_enable_subagents(),
            config_picker::ADVISOR_MODE_VALUE => self.toggle_advisor_mode(agent).await,
            config_picker::ADVISOR_MODEL_VALUE => {
                self.open_advisor_model_prompt(
                    super::agent_picker::InternalAgentModelPickerOrigin::AdvisorModelConfigRow,
                );
                Ok(())
            }
            config_picker::ADVISOR_REASONING_VALUE => self.cycle_advisor_reasoning(agent).await,
            config_picker::AUTO_COMPACT_VALUE => self.toggle_auto_compact(),
            config_picker::COMPACT_THRESHOLD_PERCENT_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.input_ui.set_composer(ComposerMode::ConfigNumberInput(
                    ConfigNumberInput::new(
                        ConfigNumberKey::CompactThresholdPercent,
                        config.compact_threshold_percent as usize,
                    ),
                ));
                self.set_status("edit compact threshold percent");
                Ok(())
            }
            config_picker::COMPACT_TARGET_PERCENT_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.input_ui.set_composer(ComposerMode::ConfigNumberInput(
                    ConfigNumberInput::new(
                        ConfigNumberKey::CompactTargetPercent,
                        config.compact_target_percent as usize,
                    ),
                ));
                self.set_status("edit compact target percent");
                Ok(())
            }
            config_picker::MAX_OUTPUT_BYTES_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.input_ui.set_composer(ComposerMode::ConfigNumberInput(
                    ConfigNumberInput::new(
                        ConfigNumberKey::MaxOutputBytes,
                        config.max_output_bytes,
                    ),
                ));
                self.set_status("edit max output bytes");
                Ok(())
            }
            config_picker::MAX_TOOL_OUTPUT_LINES_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.input_ui.set_composer(ComposerMode::ConfigNumberInput(
                    ConfigNumberInput::new(
                        ConfigNumberKey::MaxToolOutputLines,
                        config.max_tool_output_lines,
                    ),
                ));
                self.set_status("edit max tool output lines");
                Ok(())
            }
            config_picker::INLINE_SHELL_VALUE => {
                let config = self.info.services.config_repository.load()?;
                let child = config_picker::inline_shell_picker(&config);
                self.open_child_picker(child);
                Ok(())
            }
            config_picker::EDIT_TOOL_VALUE => {
                let config = self.info.services.config_repository.load()?;
                self.open_child_picker(config_picker::edit_tool_picker(config.edit_tool));
                Ok(())
            }
            value if value.starts_with(config_picker::EDIT_TOOL_PREFIX) => {
                let selected = &value[config_picker::EDIT_TOOL_PREFIX.len()..];
                let edit_tool: crate::config::EditTool =
                    selected.parse().map_err(anyhow::Error::msg)?;
                self.apply_edit_tool(edit_tool, agent).await?;
                let status = self.status().to_string();
                self.open_main_config_picker_selected(config_picker::EDIT_TOOL_VALUE)?;
                self.set_status(status);
                Ok(())
            }
            value if value.starts_with(config_picker::INLINE_SHELL_PREFIX) => {
                let shell = value[config_picker::INLINE_SHELL_PREFIX.len()..].to_string();
                self.info.services.config_repository.update(|config| {
                    config.inline_shell.clone_from(&shell);
                })?;
                self.open_main_config_picker_selected(config_picker::INLINE_SHELL_VALUE)?;
                self.set_status(format!("inline shell: {shell}"));
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
            config_picker::WEB_SEARCH_HOSTED_VALUE => self.toggle_web_search_hosted(),
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
        let return_picker = match self.input_ui.take_composer() {
            ComposerMode::Picker(picker) => Some(picker),
            composer => {
                self.input_ui.set_composer(composer);
                None
            }
        };
        let mut input = super::text_input::TextInput::config_api_key(key, value);
        if let Some(picker) = return_picker {
            input = input.with_return_picker(picker);
        }
        self.input_ui.set_composer(ComposerMode::TextInput(input));
        self.set_status(format!("edit {}", key.label()));
        Ok(())
    }

    pub(super) fn refresh_main_config_picker(
        &mut self,
        selected_value: &str,
    ) -> anyhow::Result<()> {
        let filter = match self.input_ui.composer() {
            ComposerMode::Picker(picker) => picker.filter.clone(),
            _ => String::new(),
        };
        self.open_main_config_picker(selected_value, filter)
    }

    /// Refresh the open config picker after a toggle, if the user is still in it.
    fn refresh_main_config_picker_if_open(&mut self, selected_value: &str) -> anyhow::Result<()> {
        if matches!(
            self.input_ui.composer(),
            ComposerMode::Picker(picker) if picker.action == PickerAction::Config
        ) {
            self.refresh_main_config_picker(selected_value)?;
        }
        Ok(())
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
            self.input_ui.set_composer(ComposerMode::Picker(root));
            self.set_status("config");
            return Ok(());
        };

        Self::restore_picker_position(&mut root, category, String::new());
        let mut picker = config_picker::category_picker(category, &self.info.runtime, &config)
            .expect("known config category must have a picker")
            .with_parent(root);
        Self::restore_picker_position(&mut picker, selected_value, filter);
        self.set_status_quiet(picker.title.clone());
        self.input_ui.set_composer(ComposerMode::Picker(picker));
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
        let (filter, parent) = match self.input_ui.composer_mut() {
            ComposerMode::Picker(picker) => (picker.filter.clone(), picker.take_parent()),
            ComposerMode::TextInput(input) => match input.take_return_picker() {
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
        self.input_ui.set_composer(ComposerMode::Picker(picker));
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
                self.refresh_main_config_picker_if_open(config_picker::CHECK_FOR_UPDATES_VALUE)?;
                self.set_status(if check_for_updates {
                    "check for updates: on"
                } else {
                    "check for updates: off"
                });
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save update check setting: {err}"
                )));
                self.refresh_main_config_picker_if_open(config_picker::CHECK_FOR_UPDATES_VALUE)?;
                self.set_status("config save failed");
            }
            Ok(
                ConfigMutation::EnableSubagents(_)
                | ConfigMutation::AutoCompact(_)
                | ConfigMutation::ShowReasoningOutput(_)
                | ConfigMutation::ZenMode(_)
                | ConfigMutation::WebSearchHosted(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        Ok(())
    }

    pub(super) fn toggle_enable_subagents(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(
            &self.info.services.config_repository,
            ConfigToggle::EnableSubagents,
        ) {
            Ok(ConfigMutation::EnableSubagents(enable_subagents)) => {
                self.refresh_main_config_picker_if_open(config_picker::ENABLE_SUBAGENTS_VALUE)?;
                self.set_status(if enable_subagents {
                    "subagents: on next session"
                } else {
                    "subagents: off next session"
                });
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save subagent setting: {err}"
                )));
                self.refresh_main_config_picker_if_open(config_picker::ENABLE_SUBAGENTS_VALUE)?;
                self.set_status("config save failed");
            }
            Ok(
                ConfigMutation::CheckForUpdates(_)
                | ConfigMutation::AutoCompact(_)
                | ConfigMutation::ShowReasoningOutput(_)
                | ConfigMutation::ZenMode(_)
                | ConfigMutation::WebSearchHosted(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        Ok(())
    }

    /// Advisor mode needs an advisor model, so turning it on from the config
    /// picker opens the model picker first and completes on selection.
    pub(super) async fn toggle_advisor_mode(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if !self.info.runtime.advisor_mode && !self.advisor_model_configured() {
            self.open_advisor_model_prompt(
                super::agent_picker::InternalAgentModelPickerOrigin::AdvisorConfigRow,
            );
            return Ok(());
        }
        // `/advisor` owns the save-and-sync transition; the config row only adds
        // a badge refresh, so both surfaces share one write path. The refresh
        // sets its own status, so the transition's status is restored after it.
        let enabled = !self.info.runtime.advisor_mode;
        self.set_advisor_mode(enabled, agent).await?;
        let status = self.status().to_string();
        self.refresh_main_config_picker_if_open(config_picker::ADVISOR_MODE_VALUE)?;
        self.set_status(status);
        Ok(())
    }

    pub(super) async fn cycle_advisor_reasoning(
        &mut self,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(selection) = self
            .info
            .runtime
            .internal_agents
            .get(crate::agent::ADVISOR_AGENT_ID)
            .cloned()
        else {
            self.set_status("select an advisor model first");
            return Ok(());
        };
        let capabilities = crate::agent::internal_agent_reasoning_capabilities(&selection);
        if capabilities == rho_providers::model::ReasoningCapabilities::NotConfigurable {
            return Ok(());
        }
        let current = crate::tools::advisor::advisor_effective_reasoning(&selection);
        let reasoning = capabilities.next_level(current);
        self.set_advisor_reasoning(reasoning)?;
        if self.info.runtime.advisor_mode {
            self.sync_advisor_runtime(agent).await;
        }
        let status = self.status().to_string();
        self.refresh_main_config_picker_if_open(config_picker::ADVISOR_REASONING_VALUE)?;
        self.set_status(status);
        Ok(())
    }

    pub(super) fn toggle_auto_compact(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(
            &self.info.services.config_repository,
            ConfigToggle::AutoCompact,
        ) {
            Ok(ConfigMutation::AutoCompact(auto_compact)) => {
                self.refresh_main_config_picker_if_open(config_picker::AUTO_COMPACT_VALUE)?;
                self.set_status(if auto_compact {
                    "auto compact: on"
                } else {
                    "auto compact: off"
                });
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save auto compact setting: {err}"
                )));
                self.refresh_main_config_picker_if_open(config_picker::AUTO_COMPACT_VALUE)?;
                self.set_status("config save failed");
            }
            Ok(
                ConfigMutation::CheckForUpdates(_)
                | ConfigMutation::EnableSubagents(_)
                | ConfigMutation::ShowReasoningOutput(_)
                | ConfigMutation::ZenMode(_)
                | ConfigMutation::WebSearchHosted(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
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
                self.apply_reasoning_output_visibility();
                self.refresh_main_config_picker_if_open(
                    config_picker::SHOW_REASONING_OUTPUT_VALUE,
                )?;
                self.set_status(if show_reasoning_output {
                    "reasoning output: shown"
                } else {
                    "reasoning output: hidden"
                });
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save reasoning output setting: {err}"
                )));
                self.refresh_main_config_picker_if_open(
                    config_picker::SHOW_REASONING_OUTPUT_VALUE,
                )?;
                self.set_status("config save failed");
            }
            Ok(
                ConfigMutation::CheckForUpdates(_)
                | ConfigMutation::EnableSubagents(_)
                | ConfigMutation::AutoCompact(_)
                | ConfigMutation::ZenMode(_)
                | ConfigMutation::WebSearchHosted(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        Ok(())
    }

    pub(super) fn toggle_zen_mode(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(&self.info.services.config_repository, ConfigToggle::ZenMode) {
            Ok(ConfigMutation::ZenMode(zen_mode)) => {
                self.info.runtime.zen_mode = zen_mode;
                // Zen is pure display policy over existing history; rebuild layout.
                self.history.invalidate_from(0);
                self.apply_reasoning_output_visibility();
                self.refresh_main_config_picker_if_open(config_picker::ZEN_MODE_VALUE)?;
                self.set_status(if zen_mode {
                    "zen mode: on"
                } else {
                    "zen mode: off"
                });
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save zen mode setting: {err}"
                )));
                self.refresh_main_config_picker_if_open(config_picker::ZEN_MODE_VALUE)?;
                self.set_status("config save failed");
            }
            Ok(
                ConfigMutation::CheckForUpdates(_)
                | ConfigMutation::EnableSubagents(_)
                | ConfigMutation::AutoCompact(_)
                | ConfigMutation::ShowReasoningOutput(_)
                | ConfigMutation::WebSearchHosted(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        Ok(())
    }

    pub(super) fn toggle_web_search_hosted(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(
            &self.info.services.config_repository,
            ConfigToggle::WebSearchHosted,
        ) {
            Ok(ConfigMutation::WebSearchHosted(hosted)) => {
                self.set_status(if hosted {
                    "hosted web search: on next session"
                } else {
                    "hosted web search: off next session"
                });
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save hosted web search setting: {err}"
                )));
                self.set_status("config save failed");
            }
            Ok(
                ConfigMutation::CheckForUpdates(_)
                | ConfigMutation::EnableSubagents(_)
                | ConfigMutation::AutoCompact(_)
                | ConfigMutation::ShowReasoningOutput(_)
                | ConfigMutation::ZenMode(_)
                | ConfigMutation::WebSearchProvider(_),
            ) => unreachable!("toggle returned a mismatched config mutation"),
        }
        self.refresh_web_search_config_picker(config_picker::WEB_SEARCH_HOSTED_VALUE)?;
        Ok(())
    }

    pub(super) fn cycle_web_search_provider(&mut self) -> anyhow::Result<()> {
        let ConfigMutation::WebSearchProvider(provider) =
            config_editor::cycle_web_search_provider(&self.info.services.config_repository)?
        else {
            unreachable!("provider cycle returned a mismatched config mutation");
        };
        self.refresh_web_search_config_picker(config_picker::WEB_SEARCH_PROVIDER_VALUE)?;
        self.set_status(format!("backup web search: {provider}"));
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

    /// Saves the edit-tool preference and applies the resolved format when possible.
    ///
    /// The system prompt stays fixed. A successful live switch rebuilds the tool
    /// list for the next turn and appends a model-facing schema notice.
    /// [`crate::config::EditTool::Auto`] keeps `auto` in config and advertises the
    /// preferred format for the active provider.
    pub(super) async fn apply_edit_tool(
        &mut self,
        edit_tool: crate::config::EditTool,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        let resolved = edit_tool.resolve(&self.info.runtime.provider);
        let previous = match agent.set_edit_tool(resolved, config.max_output_bytes).await {
            Ok(previous) => previous,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not apply edit tool: {error}")));
                self.set_status("edit tool change failed");
                return Ok(());
            }
        };

        if let Err(error) = self.info.services.config_repository.update(|config| {
            config.edit_tool = edit_tool;
        }) {
            if let Some(previous) = previous {
                if let Err(rollback_error) =
                    agent.set_edit_tool(previous, config.max_output_bytes).await
                {
                    return Err(anyhow::anyhow!(
                        "could not save edit tool: {error}; runtime rollback failed: {rollback_error}"
                    ));
                }
            }
            self.insert_entry(&Entry::Error(format!(
                "could not save edit tool setting: {error}"
            )));
            self.set_status("config save failed");
            return Ok(());
        }

        self.info
            .services
            .diagnostics
            .update_edit_tool(edit_tool.as_str());
        if let Some(previous) = previous {
            match agent.notify_edit_tool_switch(previous, resolved) {
                Ok(display) => {
                    self.insert_entry(&Entry::Notice(display));
                    self.info
                        .services
                        .diagnostics
                        .update_tools(&agent.tool_specs());
                }
                Err(error) => {
                    self.insert_entry(&Entry::Error(format!(
                        "edit tool is {}, but the session notice could not be added: {error}",
                        edit_tool.display_label(&self.info.runtime.provider)
                    )));
                }
            }
        }
        self.set_status(format!(
            "edit tool: {}",
            edit_tool.display_label(&self.info.runtime.provider)
        ));
        Ok(())
    }

    /// When edit preference is Auto, advertise the preferred format for
    /// `provider`. Failures are reported as notices and do not undo a model
    /// switch.
    pub(super) async fn apply_auto_edit_tool_for_provider(
        &mut self,
        provider: &str,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        if config.edit_tool != crate::config::EditTool::Auto {
            return Ok(());
        }
        let resolved = config.edit_tool.resolve(provider);
        let previous = match agent.set_edit_tool(resolved, config.max_output_bytes).await {
            Ok(previous) => previous,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "model switched, but auto edit tool could not follow the provider: {error}"
                )));
                return Ok(());
            }
        };
        let Some(previous) = previous else {
            return Ok(());
        };
        match agent.notify_edit_tool_switch(previous, resolved) {
            Ok(display) => {
                self.insert_entry(&Entry::Notice(display));
                self.info
                    .services
                    .diagnostics
                    .update_tools(&agent.tool_specs());
                self.set_status(format!(
                    "edit tool: {}",
                    config.edit_tool.display_label(provider)
                ));
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "auto edit tool is {}, but the session notice could not be added: {error}",
                    resolved.as_str()
                )));
            }
        }
        Ok(())
    }

    pub(super) fn reject_edit_tool_change(&mut self) {
        self.set_status("edit tool cannot change until the current turn finishes");
    }
}
