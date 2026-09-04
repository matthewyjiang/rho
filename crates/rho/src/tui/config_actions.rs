use ratatui::DefaultTerminal;
use rho_providers::credentials::load_web_search_api_key;

use super::{
    config_editor, config_picker,
    config_row::{ConfigCommitCtx, ConfigRow},
    resolve_web_search_editor_value, App, ComposerMode, ConfigNumberInput, ConfigNumberKey,
    ConfigTextKey, ConfigToggle, Entry, InteractiveRuntime,
};

/// Static description of one boolean `/config` row.
///
/// Every field here is data the row differs by; the shared save/refresh/report
/// flow lives in [`App::apply_config_toggle`].
struct BooleanConfigRow {
    toggle: ConfigToggle,
    /// Picker row to reselect and redraw after the save attempt.
    picker_value: &'static str,
    on_status: &'static str,
    off_status: &'static str,
    /// Reads as "could not save {error_noun} setting".
    error_noun: &'static str,
}

impl App {
    pub(super) async fn submit_config_selection(
        &mut self,
        value: &str,
        agent: &mut InteractiveRuntime,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let Some(row) = ConfigRow::parse(value) else {
            return Ok(());
        };
        self.commit_config_row(row, ConfigCommitCtx::Idle { agent, terminal })
            .await
    }

    pub(super) async fn submit_config_selection_during_turn(
        &mut self,
        value: &str,
    ) -> anyhow::Result<()> {
        let Some(row) = ConfigRow::parse(value) else {
            return Ok(());
        };
        self.commit_config_row(row, ConfigCommitCtx::DuringTurn)
            .await
    }

    async fn commit_config_row(
        &mut self,
        row: ConfigRow,
        ctx: ConfigCommitCtx<'_>,
    ) -> anyhow::Result<()> {
        match (row, ctx) {
            (ConfigRow::Category(category), _) => self.open_config_category(&category),
            (ConfigRow::ConversationModel, ConfigCommitCtx::Idle { .. }) => {
                self.open_config_conversation_model_picker();
                Ok(())
            }
            (ConfigRow::ConversationModel, ConfigCommitCtx::DuringTurn) => {
                self.open_config_conversation_model_picker_during_turn();
                Ok(())
            }
            (
                ConfigRow::RefreshModelList
                | ConfigRow::RefreshModelsDev
                | ConfigRow::ProviderLogin
                | ConfigRow::ProviderLogout
                | ConfigRow::SwitchAuthMode,
                ConfigCommitCtx::DuringTurn,
            ) => {
                self.set_status(
                    "provider configuration is unavailable while a model turn is running",
                );
                Ok(())
            }
            (ConfigRow::RefreshModelList, ConfigCommitCtx::Idle { .. }) => {
                self.open_config_refresh_model_picker();
                Ok(())
            }
            (ConfigRow::RefreshModelsDev, ConfigCommitCtx::Idle { terminal, agent }) => self
                .refresh_models_dev_catalog(terminal, agent)
                .await
                .map(|_| ()),
            (ConfigRow::ProviderLogin, ConfigCommitCtx::Idle { .. }) => {
                self.open_config_login_picker();
                Ok(())
            }
            (ConfigRow::ProviderLogout, ConfigCommitCtx::Idle { .. }) => {
                self.open_config_logout_picker().await
            }
            (ConfigRow::SwitchAuthMode, ConfigCommitCtx::Idle { .. }) => {
                self.open_config_auth_mode_picker()
            }
            (ConfigRow::PermissionMode, ConfigCommitCtx::Idle { .. }) => {
                let child =
                    config_picker::permission_mode_picker(self.info.runtime.permission_mode);
                self.open_child_picker(child);
                Ok(())
            }
            (
                ConfigRow::PermissionMode | ConfigRow::PermissionModeChoice(_),
                ConfigCommitCtx::DuringTurn,
            ) => {
                self.reject_permission_mode_change();
                Ok(())
            }
            (ConfigRow::PermissionModeChoice(selected), ConfigCommitCtx::Idle { agent, .. }) => {
                let mode = selected.parse()?;
                self.select_permission_mode_from_config(mode, agent).await
            }
            (ConfigRow::Reasoning, ConfigCommitCtx::Idle { agent, .. }) => {
                self.cycle_reasoning(agent).await
            }
            (ConfigRow::Reasoning, ConfigCommitCtx::DuringTurn) => {
                self.set_status("reasoning changes are unavailable while a model turn is running");
                Ok(())
            }
            (ConfigRow::ShowReasoningOutput, _) => self.toggle_reasoning_output(),
            (ConfigRow::ZenMode, _) => self.toggle_zen_mode(),
            (ConfigRow::Theme, _) => self.open_theme_picker_from_config(),
            (ConfigRow::CheckForUpdates, _) => self.toggle_check_for_updates(),
            (ConfigRow::EnableSubagents, _) => self.toggle_enable_subagents(),
            (
                ConfigRow::AdvisorMode | ConfigRow::AdvisorModel | ConfigRow::AdvisorReasoning,
                ConfigCommitCtx::DuringTurn,
            ) => {
                self.set_status("advisor settings are unavailable while a model turn is running");
                Ok(())
            }
            (ConfigRow::AdvisorMode, ConfigCommitCtx::Idle { agent, .. }) => {
                self.toggle_advisor_mode(agent).await
            }
            (ConfigRow::AdvisorModel, ConfigCommitCtx::Idle { .. }) => {
                self.open_advisor_model_prompt(
                    super::agent_picker::InternalAgentModelPickerOrigin::AdvisorModelConfigRow,
                );
                Ok(())
            }
            (ConfigRow::AdvisorReasoning, ConfigCommitCtx::Idle { agent, .. }) => {
                self.cycle_advisor_reasoning(agent).await
            }
            (
                ConfigRow::PermissionClassifierModel | ConfigRow::PermissionClassifierReasoning,
                ConfigCommitCtx::DuringTurn,
            ) => {
                self.set_status(
                    "permission classifier settings are unavailable while a model turn is running",
                );
                Ok(())
            }
            (ConfigRow::PermissionClassifierModel, ConfigCommitCtx::Idle { .. }) => {
                self.open_permission_classifier_model_prompt(
                    super::agent_picker::InternalAgentModelPickerOrigin::PermissionClassifierModelConfigRow,
                );
                Ok(())
            }
            (ConfigRow::PermissionClassifierReasoning, ConfigCommitCtx::Idle { agent, .. }) => {
                self.cycle_permission_classifier_reasoning(agent)?;
                let status = self.status().to_string();
                self.refresh_main_config_picker_if_open(
                    config_picker::PERMISSION_CLASSIFIER_REASONING_VALUE,
                )?;
                self.set_status(status);
                Ok(())
            }
            (ConfigRow::AutoCompact, _) => self.toggle_auto_compact(),
            (ConfigRow::CacheMissNotices, _) => self.toggle_cache_miss_notices(),
            (ConfigRow::Number(key), _) => self.open_config_number_editor(key),
            (ConfigRow::ClearPromptHistory, _) => self.prompt_clear_prompt_history(),
            (ConfigRow::InlineShell, ctx) => {
                let config = self.info.services.config_repository.load()?;
                self.open_child_picker(config_picker::inline_shell_picker(&config));
                if matches!(ctx, ConfigCommitCtx::DuringTurn) {
                    self.set_status("select inline shell");
                }
                Ok(())
            }
            (ConfigRow::EditTool, ctx) => {
                let config = self.info.services.config_repository.load()?;
                self.open_child_picker(config_picker::edit_tool_picker(config.edit_tool));
                if matches!(ctx, ConfigCommitCtx::DuringTurn) {
                    self.set_status("select edit tool");
                }
                Ok(())
            }
            (ConfigRow::EditToolChoice(_), ConfigCommitCtx::DuringTurn) => {
                self.reject_edit_tool_change();
                Ok(())
            }
            (ConfigRow::EditToolChoice(selected), ConfigCommitCtx::Idle { agent, .. }) => {
                let edit_tool: crate::config::EditTool =
                    selected.parse().map_err(anyhow::Error::msg)?;
                self.apply_edit_tool(edit_tool, agent).await?;
                let status = self.status().to_string();
                self.open_main_config_picker_selected(config_picker::EDIT_TOOL_VALUE)?;
                self.set_status(status);
                Ok(())
            }
            (ConfigRow::InlineShellChoice(shell), _) => {
                self.info.services.config_repository.update(|config| {
                    config.inline_shell.clone_from(&shell);
                })?;
                self.open_main_config_picker_selected(config_picker::INLINE_SHELL_VALUE)?;
                self.set_status(format!("inline shell: {shell}"));
                Ok(())
            }
            (ConfigRow::WebSearch, ctx) => {
                let config = self.info.services.config_repository.load()?;
                self.open_child_picker(config_picker::web_search_config_picker(
                    &config,
                    self.credential_store.as_ref(),
                ));
                if matches!(ctx, ConfigCommitCtx::DuringTurn) {
                    self.set_status("web search config");
                }
                Ok(())
            }
            (ConfigRow::WebSearchHosted, _) => self.toggle_web_search_hosted(),
            (ConfigRow::WebSearchProvider, _) => self.cycle_web_search_provider(),
            (ConfigRow::WebSearchApiKey(key), _) => self.open_web_search_api_key_editor(key),
            (ConfigRow::XaiImageGeneration, _) => self.toggle_xai_image_generation(),
        }
    }

    fn open_config_number_editor(&mut self, key: ConfigNumberKey) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        let value = match key {
            ConfigNumberKey::MaxOutputBytes => config.max_output_bytes,
            ConfigNumberKey::MaxToolOutputLines => config.max_tool_output_lines,
            ConfigNumberKey::CompactThresholdPercent => config.compact_threshold_percent as usize,
            ConfigNumberKey::CompactTargetPercent => config.compact_target_percent as usize,
            ConfigNumberKey::PromptHistoryLimit => {
                return self.open_prompt_history_limit_editor();
            }
            ConfigNumberKey::AgentConcurrency => {
                return self.open_agent_concurrency_editor();
            }
        };
        self.input_ui
            .set_composer(ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                key, value,
            )));
        self.set_status(format!("edit {}", key.label()));
        Ok(())
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
            ComposerMode::Picker(picker) if picker.is_config()
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
        let config = self.load_config_for_display()?;
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
        let config = self.load_config_for_display()?;
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

    /// Persist one boolean config row and reflect it in the open picker.
    ///
    /// Every boolean row shares this shape: flip the stored value, apply any
    /// live session effect, refresh the picker so the row redraws (on success
    /// *and* failure, so a failed save cannot leave a stale value on screen),
    /// then report. `apply` carries the only per-row difference that is not
    /// data: the immediate effect on live session state.
    fn apply_config_toggle(
        &mut self,
        row: BooleanConfigRow,
        apply: impl FnOnce(&mut Self, bool),
    ) -> anyhow::Result<()> {
        match config_editor::toggle(&self.info.services.config_repository, row.toggle) {
            Ok(enabled) => {
                apply(self, enabled);
                self.refresh_main_config_picker_if_open(row.picker_value)?;
                self.set_status(if enabled {
                    row.on_status
                } else {
                    row.off_status
                });
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save {} setting: {err}",
                    row.error_noun
                )));
                self.refresh_main_config_picker_if_open(row.picker_value)?;
                self.set_status("config save failed");
            }
        }
        Ok(())
    }

    pub(super) fn toggle_check_for_updates(&mut self) -> anyhow::Result<()> {
        self.apply_config_toggle(
            BooleanConfigRow {
                toggle: ConfigToggle::CheckForUpdates,
                picker_value: config_picker::CHECK_FOR_UPDATES_VALUE,
                on_status: "check for updates: on",
                off_status: "check for updates: off",
                error_noun: "update check",
            },
            |app, check_for_updates| {
                app.info
                    .services
                    .diagnostics
                    .update_check_for_updates(check_for_updates);
                if !check_for_updates {
                    app.info.services.update_notice = None;
                }
            },
        )
    }

    fn load_config_for_display(&self) -> anyhow::Result<crate::config::Config> {
        let mut config = self.info.services.config_repository.load()?;
        config.agent_concurrency = self.live_agent_concurrency(config.agent_concurrency);
        Ok(config)
    }

    fn live_agent_concurrency(&self, persisted: usize) -> usize {
        self.agent_concurrency
            .as_ref()
            .map(|pool| pool.total_limit())
            .unwrap_or(persisted)
    }

    pub(super) fn apply_live_agent_concurrency(&self, value: usize) {
        if let Some(pool) = &self.agent_concurrency {
            pool.set_total(value);
        }
        self.info
            .services
            .diagnostics
            .update_agent_concurrency(value);
    }

    pub(super) fn open_agent_concurrency_editor(&mut self) -> anyhow::Result<()> {
        let value = self.live_agent_concurrency(
            self.info
                .services
                .config_repository
                .load()
                .map(|config| config.agent_concurrency)
                .unwrap_or(crate::config::DEFAULT_AGENT_CONCURRENCY),
        );
        self.input_ui
            .set_composer(ComposerMode::ConfigNumberInput(ConfigNumberInput::new(
                ConfigNumberKey::AgentConcurrency,
                value,
            )));
        self.set_status("edit concurrent agents");
        Ok(())
    }

    pub(super) fn toggle_enable_subagents(&mut self) -> anyhow::Result<()> {
        self.apply_config_toggle(
            BooleanConfigRow {
                toggle: ConfigToggle::EnableSubagents,
                picker_value: config_picker::ENABLE_SUBAGENTS_VALUE,
                on_status: "subagents: on next session",
                off_status: "subagents: off next session",
                error_noun: "subagent",
            },
            |_, _| {},
        )
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
        self.apply_config_toggle(
            BooleanConfigRow {
                toggle: ConfigToggle::AutoCompact,
                picker_value: config_picker::AUTO_COMPACT_VALUE,
                on_status: "auto compact: on",
                off_status: "auto compact: off",
                error_noun: "auto compact",
            },
            |_, _| {},
        )
    }

    pub(super) fn toggle_cache_miss_notices(&mut self) -> anyhow::Result<()> {
        self.apply_config_toggle(
            BooleanConfigRow {
                toggle: ConfigToggle::CacheMissNotices,
                picker_value: config_picker::CACHE_MISS_NOTICES_VALUE,
                on_status: "cache miss notices: on",
                off_status: "cache miss notices: off",
                error_noun: "cache miss notices",
            },
            |app, enabled| app.info.runtime.cache_miss_notices = enabled,
        )
    }

    pub(super) fn toggle_reasoning_output(&mut self) -> anyhow::Result<()> {
        self.apply_config_toggle(
            BooleanConfigRow {
                toggle: ConfigToggle::ShowReasoningOutput,
                picker_value: config_picker::SHOW_REASONING_OUTPUT_VALUE,
                on_status: "reasoning output: shown",
                off_status: "reasoning output: hidden",
                error_noun: "reasoning output",
            },
            |app, show_reasoning_output| {
                app.info.runtime.show_reasoning_output = show_reasoning_output;
                app.apply_reasoning_output_visibility();
            },
        )
    }

    pub(super) fn toggle_zen_mode(&mut self) -> anyhow::Result<()> {
        self.apply_config_toggle(
            BooleanConfigRow {
                toggle: ConfigToggle::ZenMode,
                picker_value: config_picker::ZEN_MODE_VALUE,
                on_status: "zen mode: on",
                off_status: "zen mode: off",
                error_noun: "zen mode",
            },
            |app, zen_mode| {
                app.info.runtime.zen_mode = zen_mode;
                // Zen is pure display policy over existing history; rebuild layout.
                app.history.invalidate_from(0);
                app.apply_reasoning_output_visibility();
            },
        )
    }

    pub(super) fn toggle_xai_image_generation(&mut self) -> anyhow::Result<()> {
        self.apply_config_toggle(
            BooleanConfigRow {
                toggle: ConfigToggle::XaiImageGeneration,
                picker_value: config_picker::XAI_IMAGE_GENERATION_VALUE,
                on_status: "xAI image generation: on next session",
                off_status: "xAI image generation: off next session",
                error_noun: "xAI image generation",
            },
            |_, _| {},
        )
    }

    pub(super) fn toggle_web_search_hosted(&mut self) -> anyhow::Result<()> {
        match config_editor::toggle(
            &self.info.services.config_repository,
            ConfigToggle::WebSearchHosted,
        ) {
            Ok(hosted) => {
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
        }
        self.refresh_web_search_config_picker(config_picker::WEB_SEARCH_HOSTED_VALUE)?;
        Ok(())
    }

    pub(super) fn cycle_web_search_provider(&mut self) -> anyhow::Result<()> {
        let provider =
            config_editor::cycle_web_search_provider(&self.info.services.config_repository)?;
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
        let mut config = self.info.services.config_repository.load()?;
        let provider = self.info.runtime.provider.clone();
        config.edit_tool = edit_tool;
        config.provider.clone_from(&provider);
        let resolved = config.resolved_edit_tool();
        let change = match self
            .apply_resolved_edit_tool(agent, resolved, config.max_output_bytes, |error| {
                format!("could not apply edit tool: {error}")
            })
            .await
        {
            Ok(change) => change,
            Err(()) => {
                self.set_status("edit tool change failed");
                return Ok(());
            }
        };

        if let Err(error) = self.info.services.config_repository.update(|config| {
            config.edit_tool = edit_tool;
        }) {
            if let Some(change) = change {
                // Forward switch already landed in model-visible and persisted
                // display history. Rollback records the reverse the same way.
                // Mirror both into the transcript so UI, model context, and
                // display history describe the same transition sequence.
                match agent
                    .set_edit_tool(change.previous, config.max_output_bytes)
                    .await
                {
                    Ok(rollback) => {
                        self.insert_entry(&Entry::Notice(change.display.clone()));
                        if let Some(rollback) = rollback {
                            self.insert_entry(&Entry::Notice(rollback.display));
                        }
                    }
                    Err(rollback_error) => {
                        self.insert_entry(&Entry::Notice(change.display.clone()));
                        return Err(anyhow::anyhow!(
                            "could not save edit tool: {error}; runtime rollback failed: {rollback_error}"
                        ));
                    }
                }
            }
            self.insert_entry(&Entry::Error(format!(
                "could not save edit tool setting: {error}"
            )));
            self.set_status("config save failed");
            return Ok(());
        }

        // UI mirrors only after the preference is durable, so a save failure
        // cannot leave diagnostics/notices ahead of config.
        self.info
            .services
            .diagnostics
            .update_edit_tool(edit_tool.as_str());
        if let Some(change) = change.as_ref() {
            self.info
                .services
                .diagnostics
                .update_tools(&agent.tool_specs());
            self.insert_entry(&Entry::Notice(change.display.clone()));
        }
        self.set_status(format!("edit tool: {}", config.edit_tool_display_label()));
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
        let resolved = config.resolved_edit_tool_for_provider(provider);
        match self
            .apply_resolved_edit_tool(agent, resolved, config.max_output_bytes, |error| {
                format!("model switched, but auto edit tool could not follow the provider: {error}")
            })
            .await
        {
            Ok(Some(change)) => {
                // Auto provider-follow does not persist a preference; mirror
                // the live tool list and notice immediately. Leave the caller's
                // status toast alone (model switch should keep its own feedback).
                self.info
                    .services
                    .diagnostics
                    .update_tools(&agent.tool_specs());
                self.insert_entry(&Entry::Notice(change.display));
            }
            Ok(None) => {}
            Err(()) => {}
        }
        Ok(())
    }

    /// Applies a concrete edit format on the live runtime.
    ///
    /// On runtime failure inserts an error entry built by `on_error` and returns
    /// `Err(())`. `Ok(None)` means the advertised surface did not change.
    /// Callers that persist a preference must apply diagnostics/notice updates
    /// only after that save succeeds; the Auto provider-switch path updates UI
    /// immediately because it does not write config.
    async fn apply_resolved_edit_tool(
        &mut self,
        agent: &mut InteractiveRuntime,
        resolved: rho_tools::EditFormat,
        max_output_bytes: usize,
        on_error: impl FnOnce(&anyhow::Error) -> String,
    ) -> Result<Option<crate::app::interactive_runtime::edit_tool::EditToolChange>, ()> {
        match agent.set_edit_tool(resolved, max_output_bytes).await {
            Ok(change) => Ok(change),
            Err(error) => {
                self.insert_entry(&Entry::Error(on_error(&error)));
                Err(())
            }
        }
    }

    pub(super) fn reject_edit_tool_change(&mut self) {
        self.set_status("edit tool cannot change until the current turn finishes");
    }
}

#[cfg(test)]
#[path = "config_actions_tests.rs"]
mod tests;
