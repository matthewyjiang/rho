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
        let mut config = self.info.services.config_repository.load()?;
        config.provider.clone_from(&self.info.runtime.provider);
        config.model.clone_from(&self.info.runtime.model);
        config.auth.clone_from(&self.info.runtime.auth);
        config.reasoning = self.info.runtime.reasoning;
        let provider = crate::credential_store::build_provider_from_config_ensuring_catalog(
            &config,
            std::sync::Arc::clone(&self.credential_store),
        )
        .await?;
        agent.apply_web_search(config, provider).await?;
        self.web_search_reload_pending = false;
        self.set_status("web search settings applied");
        Ok(())
    }

    pub(in crate::tui) fn handle_web_search_action(
        &mut self,
        action: WebSearchAction,
        ctx: ConfigCommitCtx<'_>,
    ) -> anyhow::Result<()> {
        match action {
            WebSearchAction::Mode => self.cycle_web_search_mode(),
            WebSearchAction::Backend => self.cycle_web_search_backend(),
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
            WebSearchAction::OpenAiConnection => self.cycle_openai_search_connection(),
            WebSearchAction::ExaConnection => self.cycle_exa_search_connection(),
            WebSearchAction::EditUrl(field) => self.open_web_search_url_editor(field),
            WebSearchAction::ResetUrl(field) => self.reset_web_search_url(field),
            WebSearchAction::EditKey(key) => self.open_web_search_api_key_editor(key),
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
                self.credential_store.as_ref(),
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

    fn cycle_web_search_mode(&mut self) -> anyhow::Result<()> {
        let mode = self.info.services.config_repository.update(|config| {
            config.web_search.mode = config.web_search.mode.next();
            config.web_search.mode
        })?;
        self.web_search_reload_pending = true;
        self.refresh_web_search_picker(WEB_SEARCH_MODE_VALUE, None)?;
        self.set_status(format!(
            "web search mode: {}; applies next turn",
            mode.label()
        ));
        Ok(())
    }

    fn cycle_web_search_backend(&mut self) -> anyhow::Result<()> {
        let backend = self.info.services.config_repository.update(|config| {
            config.web_search.backend = config.web_search.backend.next();
            config.web_search.backend
        })?;
        self.web_search_reload_pending = true;
        self.refresh_web_search_picker(WEB_SEARCH_BACKEND_VALUE, None)?;
        self.set_status(format!(
            "web search backend: {}; applies next turn",
            backend.label()
        ));
        Ok(())
    }

    fn cycle_openai_search_connection(&mut self) -> anyhow::Result<()> {
        let connection = self.info.services.config_repository.update(|config| {
            config.web_search.openai.connection = config.web_search.openai.connection.next();
            config.web_search.openai.connection
        })?;
        self.web_search_reload_pending = true;
        self.refresh_web_search_picker(
            WEB_SEARCH_OPENAI_CONNECTION_VALUE,
            Some(SearchBackend::OpenAi),
        )?;
        self.set_status(format!(
            "OpenAI search connection: {}; applies next turn",
            connection.label()
        ));
        Ok(())
    }

    fn cycle_exa_search_connection(&mut self) -> anyhow::Result<()> {
        let connection = self.info.services.config_repository.update(|config| {
            config.web_search.exa.connection = config.web_search.exa.connection.next();
            config.web_search.exa.connection
        })?;
        self.web_search_reload_pending = true;
        self.refresh_web_search_picker(WEB_SEARCH_EXA_CONNECTION_VALUE, Some(SearchBackend::Exa))?;
        self.set_status(format!(
            "Exa search connection: {}; applies next turn",
            connection.label()
        ));
        Ok(())
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

    pub(in crate::tui) fn save_web_search_url(
        &mut self,
        field: WebSearchUrlField,
        value: &str,
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
        self.web_search_reload_pending = true;
        self.refresh_web_search_picker(field.value(), Some(field.page()))?;
        self.set_status(if parsed.is_some() {
            format!("{} saved; applies next turn", field.label())
        } else {
            format!("{} reset to default; applies next turn", field.label())
        });
        Ok(())
    }

    fn reset_web_search_url(&mut self, field: WebSearchUrlField) -> anyhow::Result<()> {
        self.info.services.config_repository.update(|config| {
            field.set(&mut config.web_search, None);
        })?;
        self.web_search_reload_pending = true;
        self.refresh_web_search_picker(field.reset_value(), Some(field.page()))?;
        self.set_status(format!(
            "{} reset to default; applies next turn",
            field.label()
        ));
        Ok(())
    }
}
