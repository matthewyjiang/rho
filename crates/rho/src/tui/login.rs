use super::{
    provider_actions::{ProviderActivation, ProviderActivationOutcome},
    InlineChoice, InlineChoiceModal, InlineChoiceOption, InlineChoicePending, *,
};
use {
    rho_providers::auth::login_dispatch::{
        AuthenticationMethod, CompletedAuthentication, InteractiveLoginCompletion,
        InteractiveLoginMode, InteractiveUserAction, ProviderAuthentication,
    },
    rho_providers::model::{provider_models::ProviderModelEndpoint, registry},
    rho_providers::provider,
};

pub(super) use super::login_secret_input::{secret_input_lines, SecretInput};

#[derive(Debug)]
pub(super) struct PendingInteractiveLogin {
    pub(super) target: LoginTarget,
    pub(super) handle: tokio::task::JoinHandle<Result<CompletedAuthentication, String>>,
}

/// What Enter in the API-key overlay resolved to.
///
/// Blank on an optional field means "do not write a new key": keep a
/// reachable key (store or env), otherwise run keyless. `/logout` is the
/// wipe path.
#[derive(Clone, Debug)]
pub(super) enum ApiKeySubmission {
    /// Store this key for the target's auth mode.
    Save { target: LoginTarget, key: String },
    /// No new key. Keep a reachable key, or run keyless when none exists.
    LeaveUnset { target: LoginTarget },
    /// Blank, but this target requires a key.
    Rejected,
}

/// Blank optional key: keep a reachable key, otherwise run keyless. Never deletes.
fn resolve_blank_optional_key(mut target: LoginTarget, has_reachable_key: bool) -> LoginTarget {
    if has_reachable_key {
        return target;
    }
    target.auth = provider::KEYLESS_AUTH.into();
    target.label = target.provider.clone();
    target
}

#[derive(Clone, Debug)]
pub(super) enum StoreChoiceNext {
    /// Continue login for a normal Rho provider after the store is chosen.
    Provider(String),
    /// Persist an already-collected API key after the store is chosen.
    SaveApiKey { target: LoginTarget, key: String },
}

fn credential_store_inline_choice(
    request: crate::credential_store::StoreChoiceRequest,
) -> anyhow::Result<InlineChoice> {
    use rho_providers::credentials::CredentialStoreBackend;

    let options = request
        .options()
        .into_iter()
        .enumerate()
        .map(|(index, option)| {
            let (label, detail) = match option.backend {
                CredentialStoreBackend::Os => (
                    "OS credential store",
                    if option.available {
                        "Recommended · system keychain / secret service".to_string()
                    } else {
                        format!("Unavailable · {}", request.detail_for(option.backend))
                    },
                ),
                CredentialStoreBackend::File => (
                    "Local file",
                    if option.available {
                        "Owner-only under ~/.rho/credentials · not encrypted at rest".to_string()
                    } else {
                        format!("Unavailable · {}", request.detail_for(option.backend))
                    },
                ),
            };
            let build = if option.available {
                InlineChoiceOption::available
            } else {
                InlineChoiceOption::unavailable
            };
            build(
                option.backend.as_str(),
                char::from(b'1' + index as u8),
                label,
                detail,
            )
            .with_alternate_shortcut(match option.backend {
                CredentialStoreBackend::Os => 'o',
                CredentialStoreBackend::File => 'f',
            })
        })
        .collect();
    InlineChoice::new(
        "Where should Rho store provider credentials?",
        "This is saved to config and used for future logins on this machine.",
        options,
    )
    .map_err(|_| {
        anyhow::anyhow!(
            "no credential store backend is available (os: {}; file: {})",
            request.os.detail,
            request.file.detail
        )
    })
}

fn selected_credential_store_backend(
    choice: &InlineChoice,
) -> rho_providers::credentials::CredentialStoreBackend {
    rho_providers::credentials::CredentialStoreBackend::parse(choice.selected_value())
        .expect("credential store choices contain valid backends")
}

impl App {
    /// Whether the external Claude Code binary currently reports signed in.
    pub(super) async fn claude_signed_in() -> bool {
        matches!(
            crate::claude_runtime::auth::query().await,
            Ok(status) if status.logged_in
        )
    }

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
        match claude_login::SignInTarget::parse(&invocation.args) {
            claude_login::SignInTarget::ClaudeCode => self.execute_claude_code_login().await,
            claude_login::SignInTarget::NewCustomHost => {
                self.start_custom_provider_onboarding();
                Ok(())
            }
            claude_login::SignInTarget::Provider(provider) => {
                self.start_login_for_provider(&provider, terminal, agent)
                    .await
            }
        }
    }

    pub(super) async fn execute_logout_command(
        &mut self,
        invocation: CommandInvocation,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if invocation.args.is_empty() {
            let claude_signed_in = Self::claude_signed_in().await;
            match provider_picker::logout_provider_picker(
                self.credential_store.as_ref(),
                claude_signed_in,
            ) {
                Ok(picker) => {
                    self.input_ui.set_composer(ComposerMode::Picker(picker));
                    self.set_status("select provider to logout");
                }
                Err(err) => {
                    self.insert_entry(&Entry::Error(err.to_string()));
                    self.set_status("logout failed");
                }
            }
            return Ok(());
        }
        match claude_login::SignInTarget::parse(&invocation.args) {
            claude_login::SignInTarget::ClaudeCode => self.execute_claude_code_logout().await,
            // Nothing is stored for a host that was never created.
            claude_login::SignInTarget::NewCustomHost => Ok(()),
            claude_login::SignInTarget::Provider(provider) => {
                self.logout_provider(&provider, agent).await
            }
        }
    }

    pub(super) fn open_login_picker(&mut self) {
        // Always show the login group picker first. Credential-store choice
        // happens only after a normal provider is selected, never for claude-code.
        self.input_ui
            .set_composer(ComposerMode::Picker(provider_picker::login_group_picker()));
        self.set_status("select provider to login");
    }

    fn begin_store_choice_if_needed(&mut self, next: StoreChoiceNext) -> bool {
        let config = match self.load_settings_for_login() {
            Ok(config) => config,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not load config before login: {err}"
                )));
                self.set_status("login failed");
                return true;
            }
        };
        let Some(request) = crate::credential_store::choice_request(&config) else {
            return false;
        };
        match credential_store_inline_choice(request) {
            Ok(choice) => {
                self.input_ui
                    .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                        choice,
                        pending: InlineChoicePending::CredentialStore { next },
                        parent_picker: None,
                    }));
                self.set_status("choose credential store before login");
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("login failed");
            }
        }
        true
    }

    fn load_settings_for_login(&self) -> anyhow::Result<crate::config::Config> {
        let path = self.info.services.config_repository.configured_path()?;
        crate::config::Config::load_settings_only(path)
    }

    pub(super) async fn submit_credential_store_choice(
        &mut self,
        choice: InlineChoice,
        next: StoreChoiceNext,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let backend = selected_credential_store_backend(&choice);
        let config_path = match self.info.services.config_repository.configured_path() {
            Ok(path) => Some(path),
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("credential store selection failed");
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
                self.set_status("credential store selection failed");
                return Ok(());
            }
        }

        match next {
            StoreChoiceNext::Provider(provider) => {
                self.start_login_for_provider(&provider, terminal, agent)
                    .await
            }
            StoreChoiceNext::SaveApiKey { target, key } => {
                self.persist_api_key_and_finish(target, key, terminal, agent)
                    .await
            }
        }
    }

    /// Begin login for a Rho provider credential.
    ///
    /// Callers resolve [`claude_login::SignInTarget`] first, so the external
    /// Claude Code runtime never reaches this path.
    pub(super) async fn start_login_for_provider(
        &mut self,
        provider: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let provider = provider.trim();
        // A fully keyless provider has no login target or group, so it would
        // otherwise fall through to the unsupported-provider error below.
        // `AuthenticationMethod::None` reports the same thing for targets that
        // do resolve.
        if provider::provider_descriptor(provider).is_some_and(|descriptor| descriptor.is_keyless())
        {
            self.report_login_not_required(provider);
            return Ok(());
        }
        // Resolve in this order:
        // 1. exact auth profile id (method picker values, `/login ollama-cloud-device`)
        // 2. unique provider login target (`/login openai` → api-key only)
        // 3. login group method picker (multi-mode providers / multi-product groups)
        //
        // Group lookup must not win over a unique provider target: the OpenAI
        // group also offers Codex, but `/login openai` means the OpenAI provider.
        if let Some(target) = catalog::login_target_for_auth(provider)
            .or_else(|| catalog::login_target_for_provider(provider))
        {
            return self.start_resolved_login(target, terminal, agent).await;
        }
        if let Some(group) = catalog::login_group(provider) {
            match super::provider_picker::login_group_next(group) {
                super::provider_picker::LoginGroupNext::Provider(value) => {
                    let Some(target) = catalog::login_target_for_auth(&value)
                        .or_else(|| catalog::login_target_for_provider(&value))
                    else {
                        self.insert_entry(&Entry::Error(format!(
                            "unsupported login provider '{value}'"
                        )));
                        self.set_status("login failed");
                        return Ok(());
                    };
                    return self.start_resolved_login(target, terminal, agent).await;
                }
                super::provider_picker::LoginGroupNext::MethodPicker(picker) => {
                    self.input_ui.set_composer(ComposerMode::Picker(*picker));
                    self.set_status(format!("select {} login method", provider));
                    return Ok(());
                }
            }
        }
        let mut providers = catalog::login_targets()
            .into_iter()
            .map(|target| format!("/login {}", target.provider))
            .collect::<Vec<_>>();
        providers.sort();
        providers.dedup();
        let providers = providers.join(", ");
        self.insert_entry(&Entry::Error(format!(
            "unsupported login provider '{provider}'. Use {providers}, /login {}",
            claude_login::CLAUDE_CODE_TARGET
        )));
        self.set_status("login failed");
        Ok(())
    }

    /// After a target is known: persist an endpoint if this host needs one,
    /// then choose a credential store only when a key may be stored.
    async fn start_resolved_login(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if provider::provider_descriptor(&target.provider)
            .is_some_and(|descriptor| descriptor.collects_login_endpoint())
        {
            self.start_endpoint_onboarding(&target.provider);
            return Ok(());
        }
        if self.begin_store_choice_if_needed(StoreChoiceNext::Provider(target.provider.clone())) {
            return Ok(());
        }
        self.start_login_for_target(target, terminal, agent).await
    }

    fn report_login_not_required(&mut self, provider: &str) {
        self.set_status(format!(
            "{provider} does not require login. Refresh its model list in /config, then choose a model with /model."
        ));
    }

    async fn start_login_for_target(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match ProviderAuthentication::method(&target.auth)
            .expect("catalog returned unsupported login provider")
        {
            AuthenticationMethod::None => {
                self.report_login_not_required(&target.provider);
                Ok(())
            }
            AuthenticationMethod::ApiKey { entry_label } => {
                // A host that also runs keyless accepts a blank key as "do not write a new key".
                let optional = provider::provider_descriptor(&target.provider)
                    .is_some_and(|descriptor| descriptor.has_none_auth());
                let (secret, status) = if optional {
                    (
                        SecretInput::optional(target),
                        "enter API key or leave blank".to_string(),
                    )
                } else {
                    (SecretInput::new(target), format!("enter {entry_label}"))
                };
                self.input_ui
                    .set_composer(ComposerMode::SecretInput(secret));
                self.set_status(status);
                Ok(())
            }
            AuthenticationMethod::Interactive { provider_label } => {
                self.start_interactive_login_flow(target, provider_label, terminal, agent)
                    .await
            }
        }
    }

    pub(super) async fn submit_api_key_login(
        &mut self,
        submission: ApiKeySubmission,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match submission {
            ApiKeySubmission::Rejected => {
                self.insert_entry(&Entry::Error("API key cannot be empty".into()));
                self.set_status("login failed");
                Ok(())
            }
            ApiKeySubmission::LeaveUnset { target } => {
                match ProviderAuthentication::has_credentials(
                    self.credential_store.as_ref(),
                    &target.auth,
                ) {
                    Ok(has_key) => {
                        self.finish_login(
                            resolve_blank_optional_key(target, has_key),
                            terminal,
                            agent,
                        )
                        .await
                    }
                    Err(err) => {
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.set_status("login failed");
                        Ok(())
                    }
                }
            }
            ApiKeySubmission::Save { target, key } => {
                if self.begin_store_choice_if_needed(StoreChoiceNext::SaveApiKey {
                    target: target.clone(),
                    key: key.clone(),
                }) {
                    return Ok(());
                }
                self.persist_api_key_and_finish(target, key, terminal, agent)
                    .await
            }
        }
    }

    async fn persist_api_key_and_finish(
        &mut self,
        target: LoginTarget,
        key: String,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        self.cancel_limits_command().await;
        let saved = ProviderAuthentication::save_api_key(
            self.credential_store.as_ref(),
            &target.auth,
            &key,
        );
        match saved {
            Ok(()) => self.finish_login(target, terminal, agent).await,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("login failed");
                Ok(())
            }
        }
    }

    async fn start_interactive_login_flow(
        &mut self,
        target: LoginTarget,
        provider_label: &'static str,
        terminal: &mut DefaultTerminal,
        _agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.pending_interactive_login.is_some() {
            self.insert_entry(&Entry::Notice(
                "Interactive login is already in progress. Press esc to cancel.".into(),
            ));
            return Ok(());
        }

        let remote_or_nested = std::env::var_os("SSH_CONNECTION").is_some()
            || std::env::var_os("SSH_TTY").is_some()
            || std::env::var_os("HERDR_ENV").is_some();
        let mode =
            if remote_or_nested && ProviderAuthentication::supports_device_login(&target.auth) {
                InteractiveLoginMode::Device
            } else {
                InteractiveLoginMode::Browser
            };
        self.set_status(match mode {
            InteractiveLoginMode::Browser => format!("starting {provider_label} login"),
            InteractiveLoginMode::Device => format!("starting {provider_label} device login"),
        });
        terminal.draw(|frame| self.draw(frame))?;
        let login = match ProviderAuthentication::start_interactive_login(&target.auth, mode).await
        {
            Ok(login) => login,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("login failed");
                return Ok(());
            }
        };

        let provider_label = login.provider_label;
        let waits_for_confirmation =
            matches!(&login.completion, InteractiveLoginCompletion::Confirm(_));
        let device_flow = matches!(&login.user_action, InteractiveUserAction::DeviceCode { .. });
        match login.user_action {
            InteractiveUserAction::BrowserOpened => {
                let cancel_hint = if waits_for_confirmation {
                    " Press esc to cancel."
                } else {
                    ""
                };
                self.insert_entry(&Entry::Notice(format!(
                    "opening browser for {provider_label} login.{cancel_hint}"
                )));
            }
            InteractiveUserAction::OpenUrl { url, instruction } => {
                self.insert_entry(&Entry::Notice(format!("{provider_label}: {instruction}")));
                self.insert_entry(&Entry::Notice(url));
            }
            InteractiveUserAction::DeviceCode {
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
        let completion = match login.completion {
            InteractiveLoginCompletion::Confirm(completion) => completion,
            InteractiveLoginCompletion::Unconfirmed { instruction } => {
                self.insert_entry(&Entry::Notice(instruction.into()));
                self.input_ui.set_composer(ComposerMode::Input);
                self.refresh_available_auths();
                self.report_resting_herdr_state().await;
                return Ok(());
            }
        };
        let flow = if device_flow { " device" } else { "" };
        self.set_status(format!(
            "waiting for {provider_label}{flow} login; press esc to cancel"
        ));
        self.input_ui
            .set_composer(ComposerMode::InteractivePending(target.clone()));
        self.pending_interactive_login = Some(PendingInteractiveLogin {
            target,
            handle: tokio::spawn(async move { completion.await.map_err(|err| err.to_string()) }),
        });
        Ok(())
    }

    pub(super) async fn poll_pending_interactive_login(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let Some(pending) = self.pending_interactive_login.as_ref() else {
            return Ok(());
        };
        if !pending.handle.is_finished() {
            return Ok(());
        }

        let pending = self.pending_interactive_login.take().unwrap();
        let target = pending.target;
        match pending.handle.await {
            Ok(Ok(result)) => {
                self.cancel_limits_command().await;
                let saved = result.save(self.credential_store.as_ref());
                match saved {
                    Ok(()) => {
                        self.input_ui.set_composer(ComposerMode::Input);
                        self.finish_login(target, terminal, agent).await
                    }
                    Err(err) => {
                        self.input_ui.set_composer(ComposerMode::Input);
                        self.insert_entry(&Entry::Error(err.to_string()));
                        self.set_status("login failed");
                        Ok(())
                    }
                }
            }
            Ok(Err(err)) => {
                self.input_ui.set_composer(ComposerMode::Input);
                self.insert_entry(&Entry::Error(err));
                self.set_status("login failed");
                Ok(())
            }
            Err(err) if err.is_cancelled() => {
                self.input_ui.set_composer(ComposerMode::Input);
                self.set_status("login cancelled");
                Ok(())
            }
            Err(_) => {
                self.input_ui.set_composer(ComposerMode::Input);
                self.insert_entry(&Entry::Error(
                    "background task failed: interactive login".into(),
                ));
                self.set_status("login failed");
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
        // Write the keyed profile once. Later activate/reload failures must not
        // leave a stored custom key behind as `auth = "none"`.
        self.persist_login_auth(&target);
        self.refresh_available_auths();
        self.refresh_model_list_after_login(&target, terminal)
            .await?;
        if self.using_unavailable_provider {
            if self.activate_provider_after_login(&target, agent).await? {
                self.set_status(format!(
                    "stored credentials for {} and selected {}",
                    target.provider,
                    rho_providers::provider::model_reference(
                        &self.info.runtime.provider,
                        &self.info.runtime.model,
                    )
                ));
            }
        } else if target.provider == self.info.runtime.provider {
            if self
                .reload_active_provider_after_login(&target, agent)
                .await?
            {
                self.set_status(format!(
                    "stored credentials for {} and refreshed the active provider. Switch models with /model when you want to use another provider.",
                    target.provider
                ));
            }
        } else if target.auth == "none" {
            self.set_status(format!(
                "{} is ready. Switch models with /model when you want to use it.",
                target.provider
            ));
        } else {
            self.set_status(format!(
                "stored credentials for {}. Switch models with /model when you want to use it.",
                target.provider
            ));
        }
        self.announce_held_prompt_after_login();
        self.advance_setup_screen_after_login(terminal);
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

        self.set_status(format!("refreshing {} model list", target.provider));
        terminal.draw(|frame| self.draw(frame))?;
        let config = self.info.services.config_repository.load()?;
        let endpoint = config.resolved_provider_endpoint(&target.provider);
        let model_endpoint = endpoint.as_ref().map_or(
            ProviderModelEndpoint::ProviderOwned,
            ProviderModelEndpoint::OpenAiCompatible,
        );
        match refresh_provider_models_with_store(
            &target.provider,
            &target.auth,
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
                self.set_status("login saved");
                None
            }
        }
    }

    async fn reload_active_provider_after_login(
        &mut self,
        target: &LoginTarget,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let provider = self.info.runtime.provider.clone();
        let model = self.info.runtime.model.clone();
        let Some(reasoning) = self.resolve_reasoning_after_login(&provider, &model) else {
            return Ok(false);
        };
        let new_provider = match self
            .build_provider_for_selection(&provider, &model, reasoning.effective, &target.auth)
            .await
        {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "stored credentials, but could not refresh {}: {err}",
                    target.provider
                )));
                self.set_status("login saved");
                return Ok(false);
            }
        };

        let activation = ProviderActivation {
            provider,
            model,
            reasoning,
            auth: target.auth.clone(),
            replacement: new_provider,
        };
        match self.activate_provider(activation, agent)? {
            ProviderActivationOutcome::Saved => self.set_status("login saved"),
            ProviderActivationOutcome::ConfigSaveFailed(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "login applied, but saving config failed: {err}"
                )));
                self.set_status("config save failed");
            }
        }
        Ok(true)
    }

    /// Writes the login target's auth profile so a stored custom key is not
    /// left behind as `auth = "none"` after restart.
    fn persist_login_auth(&mut self, target: &LoginTarget) {
        if target.auth == provider::KEYLESS_AUTH {
            return;
        }
        let result = if target.provider == self.info.runtime.provider {
            self.info.runtime.auth = target.auth.clone();
            self.save_current_config()
        } else {
            self.info.services.config_repository.update(|config| {
                if config.provider == target.provider {
                    config.auth = target.auth.clone();
                }
            })
        };
        if let Err(err) = result {
            self.insert_entry(&Entry::Error(format!(
                "stored credentials, but saving auth mode failed: {err}"
            )));
        }
    }

    async fn activate_provider_after_login(
        &mut self,
        target: &LoginTarget,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<bool> {
        let Some(model) = catalog::default_model_for_provider(&target.provider) else {
            self.set_status(format!(
                "stored credentials for {}, but no cached models are available. Open /config and choose Refresh model lists before switching to it.",
                target.provider
            ));
            return Ok(false);
        };
        let Some(reasoning) = self.resolve_reasoning_after_login(&target.provider, &model) else {
            return Ok(false);
        };
        let new_provider = match self
            .build_provider_for_selection(
                &target.provider,
                &model,
                reasoning.effective,
                &target.auth,
            )
            .await
        {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "stored credentials, but could not activate {}: {err}",
                    target.provider
                )));
                self.set_status("login saved");
                return Ok(false);
            }
        };

        let activation = ProviderActivation {
            provider: target.provider.clone(),
            model,
            reasoning,
            auth: target.auth.clone(),
            replacement: new_provider,
        };
        match self.activate_provider(activation, agent)? {
            ProviderActivationOutcome::Saved => {
                self.set_status(format!(
                    "model: {}",
                    rho_providers::provider::model_reference(
                        &self.info.runtime.provider,
                        &self.info.runtime.model,
                    )
                ));
            }
            ProviderActivationOutcome::ConfigSaveFailed(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "selected {}, but saving config failed: {err}",
                    rho_providers::provider::model_reference(
                        &self.info.runtime.provider,
                        &self.info.runtime.model,
                    )
                )));
                self.set_status("config save failed");
            }
        }
        Ok(true)
    }

    /// Delete a Rho provider credential. See [`Self::start_login_for_provider`]
    /// for why the external runtime cannot arrive here.
    pub(super) async fn logout_provider(
        &mut self,
        provider: &str,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let provider = provider.trim();
        let Some(target) = catalog::login_target_for_provider(provider) else {
            self.insert_entry(&Entry::Error(format!(
                "unsupported logout provider '{provider}'. Use /logout {}, /logout {}",
                catalog::implemented_providers().join(", /logout "),
                claude_login::CLAUDE_CODE_TARGET
            )));
            self.set_status("logout failed");
            return Ok(());
        };

        self.cancel_limits_command().await;
        let deleted = ProviderAuthentication::delete_credentials(
            self.credential_store.as_ref(),
            &target.auth,
        );

        match deleted {
            Ok(deleted) => {
                self.refresh_available_auths();
                let env_active = ProviderAuthentication::has_environment_override(&target.auth);
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
                self.set_status("logout failed");
                Ok(())
            }
        }
    }

    fn invalidate_active_provider_if_needed(
        &mut self,
        target: &LoginTarget,
        agent: &mut InteractiveRuntime,
    ) -> bool {
        if self.info.runtime.provider != target.provider || self.info.runtime.auth != target.auth {
            self.set_status("logout complete");
            return false;
        }
        if ProviderAuthentication::has_credentials(self.credential_store.as_ref(), &target.auth)
            .unwrap_or(false)
        {
            self.set_status("logout complete");
            return false;
        }

        let error = registry::missing_credentials_error(&target.provider);
        // Credentials are gone either way; only claim the stub is active after
        // replace_provider succeeds (it rolls back on post-replace failures).
        self.info.services.auth_unavailable = Some(error.to_string());
        match agent.replace_provider(
            std::sync::Arc::new(UnavailableProvider::new(error)),
            self.info.runtime.reasoning,
            &self.info.runtime.auth,
        ) {
            Ok(_) => {
                self.using_unavailable_provider = true;
                self.set_status("no providers configured; run /login");
            }
            Err(swap_error) => {
                // Runtime still holds the prior provider object.
                self.using_unavailable_provider = false;
                self.insert_entry(&Entry::Error(format!(
                    "could not detach the logged-out provider: {swap_error}"
                )));
                self.set_status("logout complete; provider detach failed");
            }
        }
        true
    }
}

pub(super) fn interactive_pending_lines(
    target: &LoginTarget,
    width: usize,
) -> Vec<ratatui::text::Line<'static>> {
    let label = if target.auth == "ollama-cloud-device" {
        format!(
            "waiting for {} device-key login  esc cancel",
            target.provider
        )
    } else {
        format!("waiting for {} login  esc cancel", target.label)
    };
    vec![styled_line(
        truncate_one_line(&label, width),
        width,
        Theme::dim(),
        LineFill::Natural,
    )]
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
