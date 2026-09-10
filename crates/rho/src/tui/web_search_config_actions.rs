//! Saved search settings and next-turn application.
use super::*;

impl App {
    pub(in crate::tui) async fn apply_pending_web_search(
        &mut self,
        agent: &mut crate::app::interactive_runtime::InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if !self.web_search_reload_pending {
            return Ok(());
        }
        self.apply_saved_web_search(agent).await?;
        self.web_search_reload_pending = false;
        Ok(())
    }

    async fn apply_saved_web_search(
        &mut self,
        agent: &mut crate::app::interactive_runtime::InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let mut config = self.info.services.config_repository.load()?;
        config.provider.clone_from(&self.info.runtime.provider);
        config.model.clone_from(&self.info.runtime.model);
        config.auth.clone_from(&self.info.runtime.auth);
        config.reasoning = self.info.runtime.reasoning;
        let hosted_changed = agent.hosted_web_search_active()
            != crate::tools::web::hosted_web_search_active(&config);
        let provider = if hosted_changed {
            Some(
                self.build_provider_for_selection(
                    &config.provider,
                    &config.model,
                    config.reasoning,
                    &config.auth,
                )
                .await?,
            )
        } else {
            None
        };
        agent.apply_web_search(config, provider).await?;
        self.set_status("web search settings applied");
        Ok(())
    }

    pub(in crate::tui) async fn handle_web_search_action(
        &mut self,
        action: WebSearchAction,
        ctx: ConfigCommitCtx<'_>,
    ) -> anyhow::Result<()> {
        match action {
            WebSearchAction::OpenChoice(kind) => self.open_web_search_choice_picker(kind, ctx),
            WebSearchAction::SelectChoice(choice) => {
                self.select_web_search_choice(choice, ctx).await
            }
            WebSearchAction::Route => {
                self.set_status("saved search route applies before the next turn");
                Ok(())
            }
            WebSearchAction::Test => self.prompt_web_search_test(),
            WebSearchAction::OpenBackend(backend) => {
                let config = self.info.services.config_repository.load()?;
                self.open_child_picker(backend_picker(
                    backend,
                    &config,
                    self.credential_store.as_ref(),
                ));
                if matches!(ctx, ConfigCommitCtx::DuringTurn) {
                    self.set_status(backend_page_title(backend));
                }
                Ok(())
            }
            WebSearchAction::CodexEndpoint => {
                self.set_status("Codex endpoint is fixed");
                Ok(())
            }
            WebSearchAction::EditUrl(field) => self.open_web_search_url_editor(field),
            WebSearchAction::ResetUrl(field) => self.reset_web_search_url(field, ctx).await,
            WebSearchAction::EditKey(credential) => self.open_web_search_api_key_editor(credential),
        }
    }

    pub(in crate::tui) fn refresh_web_search_config_picker(
        &mut self,
        selected_value: &str,
    ) -> anyhow::Result<()> {
        let action = WebSearchAction::parse(selected_value);
        let page = action.and_then(WebSearchAction::refresh_page);
        self.refresh_web_search_picker(selected_value, page)
    }

    pub(super) fn refresh_web_search_picker(
        &mut self,
        selected_value: &str,
        page: Option<SearchBackend>,
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
        let mut picker = match page {
            Some(backend) => backend_picker(backend, &config, self.credential_store.as_ref()),
            None => main_picker(
                &config,
                &self.info.runtime.provider,
                &self.info.runtime.model,
            ),
        };
        Self::restore_picker_position(&mut picker, selected_value, filter);
        if let Some(parent) = parent {
            picker = picker.with_parent(parent);
        }
        self.input_ui.set_composer(ComposerMode::Picker(picker));
        Ok(())
    }

    pub(in crate::tui) async fn persist_web_search(
        &mut self,
        ctx: ConfigCommitCtx<'_>,
        selected_value: &str,
        page: Option<SearchBackend>,
        during_turn_status: String,
    ) -> anyhow::Result<()> {
        self.refresh_web_search_picker(selected_value, page)?;
        match ctx {
            ConfigCommitCtx::Idle { agent, .. } => self.apply_saved_web_search(agent).await,
            ConfigCommitCtx::DuringTurn => {
                self.web_search_reload_pending = true;
                self.set_status(format!("{during_turn_status}; applies next turn"));
                Ok(())
            }
        }
    }

    fn open_web_search_choice_picker(
        &mut self,
        kind: WebSearchChoiceKind,
        ctx: ConfigCommitCtx<'_>,
    ) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        self.open_child_picker(picker_for_choice(kind, &config.web_search));
        if matches!(ctx, ConfigCommitCtx::DuringTurn) {
            self.set_status(kind.open_status());
        }
        Ok(())
    }

    async fn apply_web_search_choice(
        &mut self,
        ctx: ConfigCommitCtx<'_>,
        selected_value: &str,
        page: Option<SearchBackend>,
        during_turn_status: String,
    ) -> anyhow::Result<()> {
        let _ = self.pop_picker_level();
        self.persist_web_search(ctx, selected_value, page, during_turn_status)
            .await
    }

    async fn select_web_search_choice(
        &mut self,
        choice: WebSearchChoice,
        ctx: ConfigCommitCtx<'_>,
    ) -> anyhow::Result<()> {
        self.info.services.config_repository.update(|config| {
            choice.apply(&mut config.web_search);
        })?;
        self.apply_web_search_choice(
            ctx,
            choice.kind().parent_value(),
            choice.kind().page(),
            choice.status(),
        )
        .await
    }

    fn open_web_search_url_editor(&mut self, field: WebSearchUrlField) -> anyhow::Result<()> {
        let config = self.info.services.config_repository.load()?;
        let value = field
            .configured(&config.web_search)
            .unwrap_or_default()
            .to_string();
        let return_picker = match self.input_ui.take_composer() {
            ComposerMode::Picker(picker) => Some(picker),
            composer => {
                self.input_ui.set_composer(composer);
                None
            }
        };
        let mut input = crate::tui::text_input::TextInput::config_url(field, value);
        if let Some(picker) = return_picker {
            input = input.with_return_picker(picker);
        }
        self.input_ui.set_composer(ComposerMode::TextInput(input));
        self.set_status(format!("edit {}", field.label()));
        Ok(())
    }

    pub(in crate::tui) async fn save_web_search_url(
        &mut self,
        field: WebSearchUrlField,
        value: &str,
        ctx: ConfigCommitCtx<'_>,
    ) -> anyhow::Result<()> {
        let trimmed = value.trim();
        let parsed = if trimmed.is_empty() {
            None
        } else {
            match parse_search_endpoint_url(field.label(), trimmed) {
                Ok(_) => Some(trimmed.to_string()),
                Err(error) => {
                    self.insert_entry(&Entry::Error(format!(
                        "could not save {}: {error}",
                        field.label()
                    )));
                    self.set_status("config save failed");
                    return Ok(());
                }
            }
        };
        if let Err(error) = self.info.services.config_repository.update(|config| {
            field.set(&mut config.web_search, parsed.clone());
        }) {
            self.insert_entry(&Entry::Error(format!(
                "could not save {}: {error}",
                field.label()
            )));
            self.set_status("config save failed");
            return Ok(());
        }
        self.persist_web_search(
            ctx,
            field.value(),
            Some(field.page()),
            if parsed.is_some() {
                format!("{} saved", field.label())
            } else {
                format!("{} reset to default", field.label())
            },
        )
        .await
    }

    async fn reset_web_search_url(
        &mut self,
        field: WebSearchUrlField,
        ctx: ConfigCommitCtx<'_>,
    ) -> anyhow::Result<()> {
        self.info.services.config_repository.update(|config| {
            field.set(&mut config.web_search, None);
        })?;
        self.persist_web_search(
            ctx,
            field.reset_value(),
            Some(field.page()),
            format!("{} reset to default", field.label()),
        )
        .await
    }
}
