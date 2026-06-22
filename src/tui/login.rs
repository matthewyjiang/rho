use super::*;

impl App {
    pub(super) async fn execute_login_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        if invocation.args.is_empty() {
            self.open_provider_picker("login", PickerAction::LoginProvider);
            return Ok(());
        }
        self.start_login_for_provider(&invocation.args, terminal, agent)
            .await
    }

    pub(super) async fn execute_logout_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        if invocation.args.is_empty() {
            self.open_provider_picker("logout", PickerAction::LogoutProvider);
            return Ok(());
        }
        self.logout_provider(&invocation.args, terminal, agent)
            .await
    }

    pub(super) fn open_provider_picker(&mut self, verb: &str, action: PickerAction) {
        self.composer = ComposerMode::Picker(provider_picker::provider_picker(verb, action));
        self.status = format!("select provider to {verb}");
    }

    pub(super) async fn start_login_for_provider(
        &mut self,
        provider: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let provider = provider.trim();
        let Some(target) = catalog::login_target_for_provider(provider) else {
            self.insert_entry(
                terminal,
                &Entry::Error(format!(
                    "unsupported login provider '{provider}'. Use /login openai or /login openai-codex"
                )),
            )?;
            self.status = "login failed".into();
            return Ok(());
        };

        match target.provider.as_str() {
            "openai" => {
                self.composer = ComposerMode::SecretInput(SecretInput::new(target));
                self.status = "enter OpenAI API key".into();
                Ok(())
            }
            "openai-codex" => self.start_codex_login(target, terminal, agent).await,
            _ => unreachable!("catalog returned unsupported login provider"),
        }
    }

    pub(super) async fn submit_api_key_login(
        &mut self,
        target: LoginTarget,
        key: String,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        if key.trim().is_empty() {
            self.insert_entry(terminal, &Entry::Error("API key cannot be empty".into()))?;
            self.status = "login failed".into();
            return Ok(());
        }
        match save_openai_api_key(self.credential_store.as_ref(), &key) {
            Ok(()) => self.finish_login(target, terminal, agent).await,
            Err(err) => {
                self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
                self.status = "login failed".into();
                Ok(())
            }
        }
    }

    async fn start_codex_login(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
        _agent: &mut Agent,
    ) -> anyhow::Result<()> {
        if self.pending_oauth_login.is_some() {
            self.insert_entry(
                terminal,
                &Entry::Notice("Codex login is already in progress. Press esc to cancel.".into()),
            )?;
            return Ok(());
        }

        self.status = "waiting for Codex login; press esc to cancel".into();
        self.composer = ComposerMode::OAuthPending(target.clone());
        self.pending_oauth_login = Some(PendingOAuthLogin {
            target,
            handle: tokio::spawn(codex_oauth::run_codex_oauth_flow()),
        });
        self.insert_entry(
            terminal,
            &Entry::Notice("opening browser for Codex login. Press esc to cancel.".into()),
        )?;
        Ok(())
    }

    pub(super) async fn poll_pending_oauth_login(
        &mut self,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
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
            Ok(Ok(tokens)) => match save_codex_tokens(self.credential_store.as_ref(), &tokens) {
                Ok(()) => {
                    self.composer = ComposerMode::Input;
                    self.finish_login(target, terminal, agent).await
                }
                Err(err) => {
                    self.composer = ComposerMode::Input;
                    self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
                    self.status = "login failed".into();
                    Ok(())
                }
            },
            Ok(Err(err)) => {
                self.composer = ComposerMode::Input;
                self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
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
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!("Codex login task failed: {err}")),
                )?;
                self.status = "login failed".into();
                Ok(())
            }
        }
    }

    async fn finish_login(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        self.refresh_available_auths();
        if self.using_unavailable_provider {
            self.activate_provider_after_login(&target, terminal, agent)?;
            self.insert_entry(
                terminal,
                &Entry::Notice(format!(
                    "stored credentials for {} and selected {}/{}",
                    target.provider, self.info.provider, self.info.model
                )),
            )?;
        } else if target.provider == self.info.provider {
            self.reload_active_provider_after_login(&target, terminal, agent)?;
            self.insert_entry(
                terminal,
                &Entry::Notice(format!(
                    "stored credentials for {} and refreshed the active provider. Switch models with /model when you want to use another provider.",
                    target.provider
                )),
            )?;
        } else {
            self.insert_entry(
                terminal,
                &Entry::Notice(format!(
                    "stored credentials for {}. Switch models with /model when you want to use it.",
                    target.provider
                )),
            )?;
            self.status = "login saved".into();
        }
        Ok(())
    }

    fn reload_active_provider_after_login(
        &mut self,
        target: &LoginTarget,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let new_provider = match build_provider(
            &self.info.provider,
            &self.info.model,
            reasoning_config_value(&self.info.reasoning_effort),
            reasoning_config_value(&self.info.reasoning_summary),
        ) {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!(
                        "stored credentials, but could not refresh {}: {err}",
                        target.provider
                    )),
                )?;
                self.status = "login saved".into();
                return Ok(());
            }
        };

        agent.replace_provider(new_provider);
        self.info.auth = target.auth.clone();
        self.info.auth_unavailable = None;
        self.status = "login saved".into();
        Ok(())
    }

    fn activate_provider_after_login(
        &mut self,
        target: &LoginTarget,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let model = catalog::default_model_for_provider(&target.provider)
            .unwrap_or_else(|| self.info.model.clone());
        let new_provider = match build_provider(
            &target.provider,
            &model,
            reasoning_config_value(&self.info.reasoning_effort),
            reasoning_config_value(&self.info.reasoning_summary),
        ) {
            Ok(provider) => provider,
            Err(err) => {
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!(
                        "stored credentials, but could not activate {}: {err}",
                        target.provider
                    )),
                )?;
                self.status = "login saved".into();
                return Ok(());
            }
        };

        agent.replace_provider(new_provider);
        self.info.provider = target.provider.clone();
        self.info.auth = target.auth.clone();
        self.info.model = model;
        self.info.auth_unavailable = None;
        self.using_unavailable_provider = false;
        match self.save_current_config() {
            Ok(()) => {
                self.status = format!("model: {}/{}", self.info.provider, self.info.model);
            }
            Err(err) => {
                self.insert_entry(
                    terminal,
                    &Entry::Error(format!(
                        "selected {}/{}, but saving config failed: {err}",
                        self.info.provider, self.info.model
                    )),
                )?;
                self.status = "config save failed".into();
            }
        }
        Ok(())
    }

    pub(super) async fn logout_provider(
        &mut self,
        provider: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut Agent,
    ) -> anyhow::Result<()> {
        let provider = provider.trim();
        let Some(target) = catalog::login_target_for_provider(provider) else {
            self.insert_entry(
                terminal,
                &Entry::Error(format!(
                    "unsupported logout provider '{provider}'. Use /logout openai or /logout openai-codex"
                )),
            )?;
            self.status = "logout failed".into();
            return Ok(());
        };

        let deleted = match target.provider.as_str() {
            "openai" => delete_openai_api_key(self.credential_store.as_ref()),
            "openai-codex" => delete_codex_tokens(self.credential_store.as_ref()),
            _ => unreachable!("catalog returned unsupported logout provider"),
        };

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
                self.insert_entry(terminal, &Entry::Notice(message))?;
                if self.invalidate_active_provider_if_needed(&target, agent) {
                    self.insert_entry(
                        terminal,
                        &Entry::Notice(
                            "the active provider no longer has credentials. Run /login or switch with /model."
                                .into(),
                        ),
                    )?;
                }
                Ok(())
            }
            Err(err) => {
                self.insert_entry(terminal, &Entry::Error(err.to_string()))?;
                self.status = "logout failed".into();
                Ok(())
            }
        }
    }

    fn invalidate_active_provider_if_needed(
        &mut self,
        target: &LoginTarget,
        agent: &mut Agent,
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

        let error = match target.provider.as_str() {
            "openai" => ModelError::MissingApiKey,
            "openai-codex" => ModelError::MissingCodexAuth,
            other => ModelError::UnsupportedProvider(other.to_string()),
        };
        self.info.auth_unavailable = Some(error.to_string());
        self.using_unavailable_provider = true;
        agent.replace_provider(Box::new(UnavailableProvider::new(error)));
        self.status = "no providers configured; run /login".into();
        true
    }
}
