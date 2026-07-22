use super::*;
use {
    crate::credential_store::build_provider,
    rho_providers::auth::login_dispatch::{
        AuthenticationMethod, CompletedAuthentication, OAuthMode, OAuthUserAction,
        ProviderAuthentication,
    },
    rho_providers::model::{provider_models::ProviderModelEndpoint, registry},
    rho_providers::provider,
};

#[derive(Clone, Debug)]
pub(super) struct SecretInput {
    pub(super) target: LoginTarget,
    pub(super) value: String,
    pub(super) cursor: usize,
}

impl SecretInput {
    pub(super) fn new(target: LoginTarget) -> Self {
        Self {
            target,
            value: String::new(),
            cursor: 0,
        }
    }

    pub(super) fn char_len(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map(|(index, _)| index)
            .unwrap_or(self.value.len())
    }

    pub(super) fn insert_char(&mut self, ch: char) {
        let byte_index = self.byte_index(self.cursor);
        self.value.insert(byte_index, ch);
        self.cursor += 1;
    }

    pub(super) fn insert_text(&mut self, text: &str) {
        let sanitized = text.replace('\n', "");
        let byte_index = self.byte_index(self.cursor);
        self.value.insert_str(byte_index, &sanitized);
        self.cursor += sanitized.chars().count();
    }

    pub(super) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub(super) fn delete(&mut self) {
        if self.cursor >= self.char_len() {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }
}

#[derive(Debug)]
pub(super) struct PendingOAuthLogin {
    pub(super) target: LoginTarget,
    pub(super) handle: tokio::task::JoinHandle<Result<CompletedAuthentication, String>>,
}

#[derive(Clone, Debug)]
pub(super) enum StoreChoiceNext {
    OpenPicker,
    Provider(String),
}

#[derive(Clone, Debug)]
pub(super) struct CredentialStoreChoice {
    pub(super) request: crate::credential_store::StoreChoiceRequest,
    /// Index into `request.options()`; always points at an available backend.
    pub(super) active: usize,
    pub(super) next: StoreChoiceNext,
}

impl CredentialStoreChoice {
    fn new(
        request: crate::credential_store::StoreChoiceRequest,
        next: StoreChoiceNext,
    ) -> anyhow::Result<Self> {
        let active = request
            .options()
            .into_iter()
            .position(|option| option.available)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no credential store backend is available (os: {}; file: {})",
                    request.os.detail,
                    request.file.detail
                )
            })?;
        Ok(Self {
            request,
            active,
            next,
        })
    }

    fn selected_backend(&self) -> rho_providers::credentials::CredentialStoreBackend {
        self.request.options()[self.active.min(1)].backend
    }

    fn move_previous(&mut self) {
        let options = self.request.options();
        let mut index = self.active;
        while index > 0 {
            index -= 1;
            if options[index].available {
                self.active = index;
                return;
            }
        }
    }

    fn move_next(&mut self) {
        let options = self.request.options();
        let mut index = self.active;
        while index + 1 < options.len() {
            index += 1;
            if options[index].available {
                self.active = index;
                return;
            }
        }
    }

    /// Select backend by stable shortcut: 1/o = OS, 2/f = file.
    fn select_shortcut(
        &mut self,
        key: char,
    ) -> Option<rho_providers::credentials::CredentialStoreBackend> {
        use rho_providers::credentials::CredentialStoreBackend;

        let backend = match key.to_ascii_lowercase() {
            '1' | 'o' => CredentialStoreBackend::Os,
            '2' | 'f' => CredentialStoreBackend::File,
            _ => return None,
        };
        let index = self
            .request
            .options()
            .into_iter()
            .position(|option| option.backend == backend && option.available)?;
        self.active = index;
        Some(backend)
    }
}

impl App {
    pub(super) async fn execute_login_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if invocation.args.is_empty() {
            self.open_login_picker();
            return Ok(());
        }
        self.start_login_for_provider(&invocation.args, terminal, agent)
            .await
    }

    pub(super) async fn execute_logout_command(
        &mut self,
        invocation: CommandInvocation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if invocation.args.is_empty() {
            match provider_picker::logout_provider_picker(self.credential_store.as_ref()) {
                Ok(picker) => {
                    self.composer = ComposerMode::Picker(picker);
                    self.status = "select provider to logout".into();
                }
                Err(err) => {
                    self.insert_entry(&Entry::Error(err.to_string()));
                    self.status = "logout failed".into();
                }
            }
            return Ok(());
        }
        self.logout_provider(&invocation.args, agent).await
    }

    pub(super) fn open_login_picker(&mut self) {
        if self.begin_store_choice_if_needed(StoreChoiceNext::OpenPicker) {
            return;
        }
        self.composer = ComposerMode::Picker(provider_picker::login_group_picker());
        self.status = "select provider to login".into();
    }

    fn begin_store_choice_if_needed(&mut self, next: StoreChoiceNext) -> bool {
        let config = match self.load_settings_for_login() {
            Ok(config) => config,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not load config before login: {err}"
                )));
                self.status = "login failed".into();
                return true;
            }
        };
        let Some(request) = crate::credential_store::choice_request(&config) else {
            return false;
        };
        match CredentialStoreChoice::new(request, next) {
            Ok(choice) => {
                self.composer = ComposerMode::CredentialStoreChoice(choice);
                self.status = "choose credential store before login".into();
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "login failed".into();
            }
        }
        true
    }

    fn load_settings_for_login(&self) -> anyhow::Result<crate::config::Config> {
        let path = self.info.services.config_repository.configured_path()?;
        crate::config::Config::load_settings_only(path)
    }

    pub(super) async fn handle_credential_store_choice_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        use crossterm::event::{KeyCode, KeyModifiers};

        if !matches!(self.composer, ComposerMode::CredentialStoreChoice(_)) {
            return Ok(false);
        }

        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter | KeyCode::Char(' ')) => {
                self.submit_credential_store_choice(terminal, agent).await?;
                Ok(true)
            }
            (KeyModifiers::NONE, KeyCode::Char(key @ ('1' | '2' | 'o' | 'O' | 'f' | 'F'))) => {
                let selected =
                    if let ComposerMode::CredentialStoreChoice(choice) = &mut self.composer {
                        choice.select_shortcut(key)
                    } else {
                        None
                    };
                if selected.is_some() {
                    self.submit_credential_store_choice(terminal, agent).await?;
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Esc) => {
                self.composer = ComposerMode::Input;
                self.status = if self.running { "running" } else { "ready" }.into();
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Up | KeyCode::Left) => {
                if let ComposerMode::CredentialStoreChoice(choice) = &mut self.composer {
                    choice.move_previous();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            (_, KeyCode::Down | KeyCode::Right) => {
                if let ComposerMode::CredentialStoreChoice(choice) = &mut self.composer {
                    choice.move_next();
                }
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
            _ => {
                self.paste_burst.clear();
                self.ctrl_c_streak = 0;
                Ok(true)
            }
        }
    }

    async fn submit_credential_store_choice(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let ComposerMode::CredentialStoreChoice(choice) =
            std::mem::replace(&mut self.composer, ComposerMode::Input)
        else {
            return Ok(());
        };
        let backend = choice.selected_backend();
        let next = choice.next;
        let config_path = match self.info.services.config_repository.configured_path() {
            Ok(path) => Some(path),
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "credential store selection failed".into();
                return Ok(());
            }
        };
        match crate::credential_store::set_backend(backend, config_path) {
            Ok(path) => {
                self.insert_entry(&Entry::Notice(format!(
                    "credential store set to {} in {}",
                    backend.as_str(),
                    path.display()
                )));
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "credential store selection failed".into();
                return Ok(());
            }
        }

        match next {
            StoreChoiceNext::Provider(provider) => {
                self.start_login_for_provider(&provider, terminal, agent)
                    .await
            }
            StoreChoiceNext::OpenPicker => {
                self.open_login_picker();
                Ok(())
            }
        }
    }

    pub(super) async fn start_login_for_provider(
        &mut self,
        provider: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.begin_store_choice_if_needed(StoreChoiceNext::Provider(provider.to_string())) {
            return Ok(());
        }
        let provider = provider.trim();
        if provider::provider_descriptor(provider)
            .is_some_and(|descriptor| descriptor.auth_kind == provider::ProviderAuthKind::None)
        {
            self.insert_entry(&Entry::Notice(format!(
                "{provider} does not require login. Refresh its model list in /config, then choose a model with /model."
            )));
            self.status = "no login required".into();
            return Ok(());
        }
        let Some(target) = catalog::login_target_for_provider(provider) else {
            let providers = catalog::login_targets()
                .into_iter()
                .map(|target| format!("/login {}", target.provider))
                .collect::<Vec<_>>()
                .join(", ");
            self.insert_entry(&Entry::Error(format!(
                "unsupported login provider '{provider}'. Use {providers}"
            )));
            self.status = "login failed".into();
            return Ok(());
        };

        match ProviderAuthentication::method(&target.provider)
            .expect("catalog returned unsupported login provider")
        {
            AuthenticationMethod::None => {
                self.insert_entry(&Entry::Notice(format!(
                    "{} does not require login. Refresh its model list in /config, then choose a model with /model.",
                    target.provider
                )));
                self.status = "no login required".into();
                Ok(())
            }
            AuthenticationMethod::ApiKey { entry_label } => {
                self.composer = ComposerMode::SecretInput(SecretInput::new(target));
                self.status = format!("enter {entry_label}");
                Ok(())
            }
            AuthenticationMethod::OAuth { provider_label } => {
                self.start_oauth_login(target, provider_label, terminal, agent)
                    .await
            }
        }
    }

    pub(super) async fn submit_api_key_login(
        &mut self,
        target: LoginTarget,
        key: String,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if key.trim().is_empty() {
            self.insert_entry(&Entry::Error("API key cannot be empty".into()));
            self.status = "login failed".into();
            return Ok(());
        }
        self.cancel_limits_command().await;
        let saved = ProviderAuthentication::save_api_key(
            self.credential_store.as_ref(),
            &target.provider,
            &key,
        );
        match saved {
            Ok(()) => self.finish_login(target, terminal, agent).await,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "login failed".into();
                Ok(())
            }
        }
    }

    async fn start_oauth_login(
        &mut self,
        target: LoginTarget,
        provider_label: &'static str,
        terminal: &mut DefaultTerminal,
        _agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.pending_oauth_login.is_some() {
            self.insert_entry(&Entry::Notice(
                "OAuth login is already in progress. Press esc to cancel.".into(),
            ));
            return Ok(());
        }

        let remote_or_nested = std::env::var_os("SSH_CONNECTION").is_some()
            || std::env::var_os("SSH_TTY").is_some()
            || std::env::var_os("HERDR_ENV").is_some();
        let mode = if remote_or_nested
            && ProviderAuthentication::supports_device_login(&target.provider)
        {
            OAuthMode::Device
        } else {
            OAuthMode::Browser
        };
        self.status = match mode {
            OAuthMode::Browser => format!("starting {provider_label} login"),
            OAuthMode::Device => format!("starting {provider_label} device login"),
        };
        terminal.draw(|frame| self.draw(frame))?;
        let login = match ProviderAuthentication::start_oauth(&target.provider, mode).await {
            Ok(login) => login,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "login failed".into();
                return Ok(());
            }
        };

        let provider_label = login.provider_label;
        let device_flow = matches!(&login.user_action, OAuthUserAction::DeviceCode { .. });
        match login.user_action {
            OAuthUserAction::BrowserOpened => {
                self.insert_entry(&Entry::Notice(format!(
                    "opening browser for {provider_label} login. Press esc to cancel."
                )));
            }
            OAuthUserAction::DeviceCode {
                verification_uri,
                user_code,
                verification_uri_complete,
            } => {
                self.insert_entry(&Entry::Notice(format!(
                    "{provider_label} login: visit {verification_uri} and enter code {user_code}"
                )));
                if let Some(uri) = verification_uri_complete {
                    self.insert_entry(&Entry::Notice(format!(
                        "Or open this URL to continue: {uri}"
                    )));
                }
            }
        }
        let flow = if device_flow { " device" } else { "" };
        self.status = format!("waiting for {provider_label}{flow} login; press esc to cancel");
        self.composer = ComposerMode::OAuthPending(target.clone());
        self.pending_oauth_login = Some(PendingOAuthLogin {
            target,
            handle: tokio::spawn(
                async move { login.completion.await.map_err(|err| err.to_string()) },
            ),
        });
        Ok(())
    }

    pub(super) async fn poll_pending_oauth_login(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(pending) = self.pending_oauth_login.as_ref() else {
            return Ok(());
        };
        if !pending.handle.is_finished() {
            return Ok(());
        }

        let pending = self.pending_oauth_login.take().unwrap();
        let target = pending.target;
        match pending.handle.await {
            Ok(Ok(result)) => {
                self.cancel_limits_command().await;
                let saved = result.save(self.credential_store.as_ref());
                match saved {
                    Ok(()) => {
                        self.composer = ComposerMode::Input;
                        self.finish_login(target, terminal, agent).await
                    }
                    Err(err) => {
                        self.composer = ComposerMode::Input;
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.status = "login failed".into();
                        Ok(())
                    }
                }
            }
            Ok(Err(err)) => {
                self.composer = ComposerMode::Input;
                self.insert_entry(&Entry::Error(err));
                self.status = "login failed".into();
                Ok(())
            }
            Err(err) if err.is_cancelled() => {
                self.composer = ComposerMode::Input;
                self.status = "login cancelled".into();
                Ok(())
            }
            Err(err) => {
                self.composer = ComposerMode::Input;
                self.insert_entry(&Entry::Error(format!("OAuth login task failed: {err}")));
                self.status = "login failed".into();
                Ok(())
            }
        }
    }

    async fn finish_login(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.refresh_available_auths();
        self.refresh_model_list_after_login(&target, terminal)
            .await?;
        if self.using_unavailable_provider {
            if self.activate_provider_after_login(&target, agent)? {
                self.insert_entry(&Entry::Notice(format!(
                    "stored credentials for {} and selected {}",
                    target.provider,
                    rho_providers::provider::model_reference(
                        &self.info.runtime.provider,
                        &self.info.runtime.model,
                    )
                )));
            }
        } else if target.provider == self.info.runtime.provider {
            if self.reload_active_provider_after_login(&target, agent)? {
                self.insert_entry(&Entry::Notice(format!(
                        "stored credentials for {} and refreshed the active provider. Switch models with /model when you want to use another provider.",
                        target.provider
                    )),
                );
            }
        } else {
            self.insert_entry(&Entry::Notice(format!(
                "stored credentials for {}. Switch models with /model when you want to use it.",
                target.provider
            )));
            self.status = "login saved".into();
        }
        self.report_resting_herdr_state().await;
        Ok(())
    }

    async fn refresh_model_list_after_login(
        &mut self,
        target: &LoginTarget,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let Some(descriptor) = provider::provider_descriptor(&target.provider) else {
            return Ok(());
        };
        if descriptor.model_refresh.is_none() {
            return Ok(());
        }

        self.status = format!("refreshing {} model list", target.provider);
        terminal.draw(|frame| self.draw(frame))?;
        let config = self.info.services.config_repository.load()?;
        let endpoint = config.resolved_provider_endpoint(&target.provider);
        let model_endpoint = endpoint.as_ref().map_or(
            ProviderModelEndpoint::ProviderOwned,
            ProviderModelEndpoint::OpenAiCompatible,
        );
        match refresh_provider_models_with_store(
            &target.provider,
            self.credential_store.as_ref(),
            model_endpoint,
        )
        .await
        {
            Ok(refresh) => {
                self.insert_entry(&Entry::Notice(format!(
                    "refreshed {} model list: {} models",
                    refresh.provider,
                    refresh.models.len()
                )));
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "stored credentials for {}, but failed to refresh its model list: {err}",
                    target.provider
                )));
            }
        }
        Ok(())
    }

    pub(super) fn resolve_reasoning_after_login(
        &mut self,
        provider: &str,
        model: &str,
    ) -> Option<reasoning_metadata::ModelSwitchReasoningResolution> {
        let capabilities =
            rho_providers::model::models_dev::current_reasoning_capabilities(provider, model);
        match reasoning_metadata::resolve_model_switch_reasoning(
            &capabilities,
            self.info.runtime.reasoning,
            self.info.runtime.reasoning_source,
        ) {
            Ok(reasoning) => Some(reasoning),
            Err(requested) => {
                self.insert_entry(&Entry::Error(format!(
                    "stored credentials, but reasoning level '{requested}' is not supported by {}",
                    rho_providers::provider::model_reference(provider, model)
                )));
                self.status = "login saved".into();
                None
            }
        }
    }

    fn reload_active_provider_after_login(
        &mut self,
        target: &LoginTarget,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let provider = self.info.runtime.provider.clone();
        let model = self.info.runtime.model.clone();
        let Some(reasoning) = self.resolve_reasoning_after_login(&provider, &model) else {
            return Ok(false);
        };
        let new_provider = match build_provider(&provider, &model, reasoning.effective) {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "stored credentials, but could not refresh {}: {err}",
                    target.provider
                )));
                self.status = "login saved".into();
                return Ok(false);
            }
        };

        agent.replace_provider(new_provider, reasoning.effective)?;
        self.info
            .set_reasoning(reasoning.effective, reasoning.source);
        self.info.runtime.auth = target.auth.clone();
        self.info.services.auth_unavailable = None;
        self.start_model_metadata_fetch(agent);
        match self.save_current_config() {
            Ok(()) => self.status = "login saved".into(),
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "login applied, but saving config failed: {err}"
                )));
                self.status = "config save failed".into();
            }
        }
        Ok(true)
    }

    fn activate_provider_after_login(
        &mut self,
        target: &LoginTarget,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(model) = catalog::default_model_for_provider(&target.provider) else {
            self.insert_entry(&Entry::Notice(format!(
                    "stored credentials for {}, but no cached models are available. Open /config and choose Refresh model lists before switching to it.",
                    target.provider
                )),
            );
            self.status = "login saved".into();
            return Ok(false);
        };
        let Some(reasoning) = self.resolve_reasoning_after_login(&target.provider, &model) else {
            return Ok(false);
        };
        let new_provider = match build_provider(&target.provider, &model, reasoning.effective) {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "stored credentials, but could not activate {}: {err}",
                    target.provider
                )));
                self.status = "login saved".into();
                return Ok(false);
            }
        };

        agent.replace_provider(new_provider, reasoning.effective)?;
        self.info.runtime.provider = target.provider.clone();
        self.info.runtime.auth = target.auth.clone();
        self.info.runtime.model = model;
        self.info
            .set_reasoning(reasoning.effective, reasoning.source);
        self.info.services.auth_unavailable = None;
        self.using_unavailable_provider = false;
        self.start_model_metadata_fetch(agent);
        match self.save_current_config() {
            Ok(()) => {
                self.status = format!(
                    "model: {}",
                    rho_providers::provider::model_reference(
                        &self.info.runtime.provider,
                        &self.info.runtime.model,
                    )
                );
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "selected {}, but saving config failed: {err}",
                    rho_providers::provider::model_reference(
                        &self.info.runtime.provider,
                        &self.info.runtime.model,
                    )
                )));
                self.status = "config save failed".into();
            }
        }
        Ok(true)
    }

    pub(super) async fn logout_provider(
        &mut self,
        provider: &str,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let provider = provider.trim();
        let Some(target) = catalog::login_target_for_provider(provider) else {
            self.insert_entry(&Entry::Error(format!(
                "unsupported logout provider '{provider}'. Use /logout {}",
                catalog::implemented_providers().join(", /logout ")
            )));
            self.status = "logout failed".into();
            return Ok(());
        };

        self.cancel_limits_command().await;
        let deleted = ProviderAuthentication::delete_credentials(
            self.credential_store.as_ref(),
            &target.provider,
        );

        match deleted {
            Ok(deleted) => {
                self.refresh_available_auths();
                let env_active = ProviderAuthentication::has_environment_override(&target.provider);
                let message = if env_active {
                    format!(
                        "deleted stored credentials for {}, but an env override is still active",
                        target.provider
                    )
                } else if deleted {
                    format!("deleted stored credentials for {}", target.provider)
                } else {
                    format!("no stored credentials for {} were present", target.provider)
                };
                self.insert_entry(&Entry::Notice(message));
                if self.invalidate_active_provider_if_needed(&target, agent) {
                    self.insert_entry(&Entry::Notice(
                            "the active provider no longer has credentials. Run /login or switch with /model."
                                .into(),
                        ),
                    );
                }
                self.report_resting_herdr_state().await;
                Ok(())
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "logout failed".into();
                Ok(())
            }
        }
    }

    fn invalidate_active_provider_if_needed(
        &mut self,
        target: &LoginTarget,
        agent: &mut InteractiveRuntime,
    ) -> bool {
        if self.info.runtime.provider != target.provider {
            self.status = "logout complete".into();
            return false;
        }
        if ProviderAuthentication::has_credentials(self.credential_store.as_ref(), &target.provider)
            .unwrap_or(false)
        {
            self.status = "logout complete".into();
            return false;
        }

        let error = registry::missing_credentials_error(&target.provider);
        self.info.services.auth_unavailable = Some(error.to_string());
        self.using_unavailable_provider = true;
        let _ = agent.replace_provider(
            std::sync::Arc::new(UnavailableProvider::new(error)),
            self.info.runtime.reasoning,
        );
        self.status = "no providers configured; run /login".into();
        true
    }
}

pub(super) fn credential_store_choice_lines(
    choice: &CredentialStoreChoice,
    width: usize,
) -> Vec<Line<'static>> {
    use rho_providers::credentials::CredentialStoreBackend;

    let width = width.max(1);
    let mut lines = vec![
        styled_line(
            truncate_one_line("Where should Rho store provider credentials?", width),
            width,
            Theme::input_prompt(),
            LineFill::Natural,
        ),
        styled_line(
            truncate_one_line(
                "This is saved to config and used for future logins on this machine.",
                width,
            ),
            width,
            Theme::dim(),
            LineFill::Natural,
        ),
    ];

    for (index, option) in choice.request.options().into_iter().enumerate() {
        let selected = index == choice.active && option.available;
        let (number, label, detail) = match option.backend {
            CredentialStoreBackend::Os => (
                "1",
                "OS credential store",
                if option.available {
                    "Recommended · system keychain / secret service".to_string()
                } else {
                    format!(
                        "Unavailable · {}",
                        choice.request.detail_for(option.backend)
                    )
                },
            ),
            CredentialStoreBackend::File => (
                "2",
                "Local file",
                if option.available {
                    "Owner-only under ~/.rho/credentials · not encrypted at rest".to_string()
                } else {
                    format!(
                        "Unavailable · {}",
                        choice.request.detail_for(option.backend)
                    )
                },
            ),
        };
        let marker = if selected {
            ">"
        } else if option.available {
            " "
        } else {
            "·"
        };
        let style = if selected {
            Theme::input_prompt()
        } else if option.available {
            Theme::text()
        } else {
            Theme::dim()
        };
        lines.push(styled_line(
            truncate_one_line(&format!("{marker} [{number}] {label}"), width),
            width,
            style,
            LineFill::Natural,
        ));
        lines.push(styled_line(
            truncate_one_line(&format!("      {detail}"), width),
            width,
            Theme::dim(),
            LineFill::Natural,
        ));
    }

    lines.push(styled_line(
        truncate_one_line(
            "enter/space choose · 1/2 or o/f jump · arrows move · esc cancel",
            width,
        ),
        width,
        Theme::dim(),
        LineFill::Natural,
    ));
    lines
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
