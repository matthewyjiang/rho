use super::*;
use crate::{model::registry, provider, providers::build_sdk_provider};

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
    pub(super) handle: tokio::task::JoinHandle<Result<PendingOAuthResult, String>>,
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
        self.composer = ComposerMode::Picker(provider_picker::login_group_picker());
        self.status = "select provider to login".into();
    }

    pub(super) async fn start_login_for_provider(
        &mut self,
        provider: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let provider = provider.trim();
        let Some(target) = catalog::login_target_for_provider(provider) else {
            self.insert_entry(&Entry::Error(format!(
                "unsupported login provider '{provider}'. Use /login {}",
                catalog::implemented_providers().join(", /login ")
            )));
            self.status = "login failed".into();
            return Ok(());
        };

        let Some(descriptor) = provider::provider_descriptor(&target.provider) else {
            unreachable!("catalog returned unsupported login provider")
        };
        match descriptor.auth_kind {
            ProviderAuthKind::ApiKey { entry_label, .. } => {
                self.composer = ComposerMode::SecretInput(SecretInput::new(target));
                self.status = format!("enter {entry_label}");
                Ok(())
            }
            ProviderAuthKind::CodexOAuth { .. } => {
                self.start_codex_login(target, terminal, agent).await
            }
            ProviderAuthKind::GithubCopilotDevice { .. } => {
                self.start_github_copilot_login(target, terminal, agent)
                    .await
            }
            ProviderAuthKind::KimiOAuth { .. } => self.start_kimi_login(target, terminal).await,
            ProviderAuthKind::XaiOAuth { .. } => self.start_xai_login(target, terminal).await,
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
        let saved = save_provider_api_key(self.credential_store.as_ref(), &target.provider, &key);
        match saved {
            Ok(()) => self.finish_login(target, terminal, agent).await,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "login failed".into();
                Ok(())
            }
        }
    }

    async fn start_codex_login(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
        _agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.pending_oauth_login.is_some() {
            self.insert_entry(&Entry::Notice(
                "OAuth login is already in progress. Press esc to cancel.".into(),
            ));
            return Ok(());
        }

        if std::env::var_os("SSH_CONNECTION").is_some()
            || std::env::var_os("SSH_TTY").is_some()
            || std::env::var_os("HERDR_ENV").is_some()
        {
            self.start_codex_device_login(target, terminal).await
        } else {
            self.status = "waiting for Codex login; press esc to cancel".into();
            self.composer = ComposerMode::OAuthPending(target.clone());
            self.pending_oauth_login = Some(PendingOAuthLogin {
                target,
                handle: tokio::spawn(async {
                    codex_oauth::run_codex_oauth_flow()
                        .await
                        .map(PendingOAuthResult::Codex)
                        .map_err(|err| err.to_string())
                }),
            });
            self.insert_entry(&Entry::Notice(
                "opening browser for Codex login. Press esc to cancel.".into(),
            ));
            Ok(())
        }
    }

    async fn start_codex_device_login(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.status = "starting Codex device login".into();
        terminal.draw(|frame| self.draw(frame))?;
        let login = match codex_oauth::start_codex_device_login().await {
            Ok(login) => login,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "login failed".into();
                return Ok(());
            }
        };

        self.insert_entry(&Entry::Notice(format!(
            "Codex login: visit {} and enter code {}",
            login.verification_uri, login.user_code
        )));

        self.status = "waiting for Codex device login; press esc to cancel".into();
        self.composer = ComposerMode::OAuthPending(target.clone());
        self.pending_oauth_login = Some(PendingOAuthLogin {
            target,
            handle: tokio::spawn(async move {
                codex_oauth::complete_codex_device_login(login)
                    .await
                    .map(PendingOAuthResult::Codex)
                    .map_err(|err| err.to_string())
            }),
        });
        Ok(())
    }

    async fn start_kimi_login(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if self.pending_oauth_login.is_some() {
            self.insert_entry(&Entry::Notice(
                "OAuth login is already in progress. Press esc to cancel.".into(),
            ));
            return Ok(());
        }
        self.status = "starting Kimi device login".into();
        terminal.draw(|frame| self.draw(frame))?;
        let login = match kimi_oauth::start_kimi_device_login().await {
            Ok(login) => login,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "login failed".into();
                return Ok(());
            }
        };
        self.insert_entry(&Entry::Notice(format!(
            "Kimi login: visit {} and enter code {}",
            login.verification_uri, login.user_code
        )));
        if let Some(uri) = &login.verification_uri_complete {
            self.insert_entry(&Entry::Notice(format!(
                "Or open this URL to continue: {uri}"
            )));
        }
        self.status = "waiting for Kimi device login; press esc to cancel".into();
        self.composer = ComposerMode::OAuthPending(target.clone());
        self.pending_oauth_login = Some(PendingOAuthLogin {
            target,
            handle: tokio::spawn(async move {
                kimi_oauth::complete_kimi_device_login(login)
                    .await
                    .map(PendingOAuthResult::Kimi)
                    .map_err(|err| err.to_string())
            }),
        });
        Ok(())
    }

    async fn start_xai_login(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if self.pending_oauth_login.is_some() {
            self.insert_entry(&Entry::Notice(
                "OAuth login is already in progress. Press esc to cancel.".into(),
            ));
            return Ok(());
        }

        if std::env::var_os("SSH_CONNECTION").is_some()
            || std::env::var_os("SSH_TTY").is_some()
            || std::env::var_os("HERDR_ENV").is_some()
        {
            self.status = "starting xAI device login".into();
            terminal.draw(|frame| self.draw(frame))?;
            let login = match xai_oauth::start_xai_device_login().await {
                Ok(login) => login,
                Err(err) => {
                    self.insert_entry(&Entry::Error(err.to_string()));
                    self.status = "login failed".into();
                    return Ok(());
                }
            };
            self.insert_entry(&Entry::Notice(format!(
                "xAI login: visit {} and enter code {}",
                login.verification_uri, login.user_code
            )));
            if let Some(uri) = &login.verification_uri_complete {
                self.insert_entry(&Entry::Notice(format!(
                    "Or open this URL to continue: {uri}"
                )));
            }
            self.status = "waiting for xAI device login; press esc to cancel".into();
            self.composer = ComposerMode::OAuthPending(target.clone());
            self.pending_oauth_login = Some(PendingOAuthLogin {
                target,
                handle: tokio::spawn(async move {
                    xai_oauth::complete_xai_device_login(login)
                        .await
                        .map(PendingOAuthResult::Xai)
                        .map_err(|err| err.to_string())
                }),
            });
        } else {
            self.status = "waiting for xAI login; press esc to cancel".into();
            self.composer = ComposerMode::OAuthPending(target.clone());
            self.pending_oauth_login = Some(PendingOAuthLogin {
                target,
                handle: tokio::spawn(async {
                    xai_oauth::run_xai_oauth_flow()
                        .await
                        .map(PendingOAuthResult::Xai)
                        .map_err(|err| err.to_string())
                }),
            });
            self.insert_entry(&Entry::Notice(
                "opening browser for xAI login. Press esc to cancel.".into(),
            ));
        }
        Ok(())
    }

    async fn start_github_copilot_login(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
        _agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.pending_oauth_login.is_some() {
            self.insert_entry(&Entry::Notice(
                "OAuth login is already in progress. Press esc to cancel.".into(),
            ));
            return Ok(());
        }

        self.status = "starting GitHub Copilot device login".into();
        terminal.draw(|frame| self.draw(frame))?;
        let login = match github_copilot_device::start_github_copilot_device_login().await {
            Ok(login) => login,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.status = "login failed".into();
                return Ok(());
            }
        };

        self.insert_entry(&Entry::Notice(format!(
            "GitHub Copilot login: visit {} and enter code {}",
            login.verification_uri, login.user_code
        )));
        if let Some(uri) = &login.verification_uri_complete {
            self.insert_entry(&Entry::Notice(format!(
                "Or open this URL to continue: {uri}"
            )));
        }

        self.status = "waiting for GitHub Copilot device login; press esc to cancel".into();
        self.composer = ComposerMode::OAuthPending(target.clone());
        self.pending_oauth_login = Some(PendingOAuthLogin {
            target,
            handle: tokio::spawn(async move {
                github_copilot_device::complete_github_copilot_device_login(login)
                    .await
                    .map(PendingOAuthResult::GithubCopilot)
                    .map_err(|err| err.to_string())
            }),
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
                let saved = match result {
                    PendingOAuthResult::Codex(tokens) => {
                        save_codex_tokens(self.credential_store.as_ref(), &tokens)
                    }
                    PendingOAuthResult::GithubCopilot(tokens) => {
                        save_github_copilot_tokens(self.credential_store.as_ref(), &tokens)
                    }
                    PendingOAuthResult::Kimi(tokens) => {
                        save_kimi_tokens(self.credential_store.as_ref(), &tokens)
                    }
                    PendingOAuthResult::Xai(tokens) => {
                        save_xai_tokens(self.credential_store.as_ref(), &tokens)
                    }
                };
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
                    "stored credentials for {} and selected {}/{}",
                    target.provider, self.info.provider, self.info.model
                )));
            }
        } else if target.provider == self.info.provider {
            self.reload_active_provider_after_login(&target, agent)?;
            self.insert_entry(&Entry::Notice(format!(
                    "stored credentials for {} and refreshed the active provider. Switch models with /model when you want to use another provider.",
                    target.provider
                )),
            );
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
        match refresh_provider_models_with_store(&target.provider, self.credential_store.as_ref())
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

    fn reload_active_provider_after_login(
        &mut self,
        target: &LoginTarget,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let new_provider =
            match build_sdk_provider(&self.info.provider, &self.info.model, self.info.reasoning) {
                Ok(provider) => provider,
                Err(err) => {
                    self.insert_entry(&Entry::Error(format!(
                        "stored credentials, but could not refresh {}: {err}",
                        target.provider
                    )));
                    self.status = "login saved".into();
                    return Ok(());
                }
            };

        agent.replace_provider(new_provider, self.info.reasoning)?;
        self.info.auth = target.auth.clone();
        self.info.auth_unavailable = None;
        self.status = "login saved".into();
        Ok(())
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
        let new_provider = match build_sdk_provider(&target.provider, &model, self.info.reasoning) {
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

        agent.replace_provider(new_provider, self.info.reasoning)?;
        self.info.provider = target.provider.clone();
        self.info.auth = target.auth.clone();
        self.info.model = model;
        self.info.diagnostics.update_identity(
            &self.info.provider,
            &self.info.model,
            self.info.reasoning,
        );
        self.info.auth_unavailable = None;
        self.using_unavailable_provider = false;
        match self.save_current_config() {
            Ok(()) => {
                self.status = format!("model: {}/{}", self.info.provider, self.info.model);
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!(
                    "selected {}/{}, but saving config failed: {err}",
                    self.info.provider, self.info.model
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
        let deleted = delete_provider_credentials(self.credential_store.as_ref(), &target.provider);

        match deleted {
            Ok(deleted) => {
                self.refresh_available_auths();
                let env_active = provider_has_env_override(&target.provider);
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
        if self.info.provider != target.provider {
            self.status = "logout complete".into();
            return false;
        }
        if provider_has_credentials(self.credential_store.as_ref(), &target.provider)
            .unwrap_or(false)
        {
            self.status = "logout complete".into();
            return false;
        }

        let error = registry::missing_credentials_error(&target.provider);
        self.info.auth_unavailable = Some(error.to_string());
        self.using_unavailable_provider = true;
        let _ = agent.replace_provider(
            std::sync::Arc::new(UnavailableProvider::new(error)),
            self.info.reasoning,
        );
        self.status = "no providers configured; run /login".into();
        true
    }
}
