//! `/login` and `/logout` against a daemon session (#1662): the daemon
//! writes the credential store its sessions read, so it checks and saves a
//! key, runs a provider's interactive login, and signs out. The TUI keeps
//! what it shows: the pickers, the key prompt, the login's URL, and the
//! browser, which opens here.
//!
//! Unlike in process, a login does not swap the running provider: the
//! daemon's sessions pick the credential up at their next prompt. A
//! logout of the profile in use marks the session signed out here, so the
//! header says so and a prompt offers `/login` rather than failing; that
//! is this TUI's reading of the shared credential store, checked again
//! before each prompt it would hold back. A login does not make the
//! session's provider and model the config's defaults either: only the
//! default provider's auth profile follows it.
//! A login its provider cannot confirm (Ollama Cloud's device key) is
//! recorded by the daemon as soon as the daemon runs it. So this process
//! shows its prompt (`login_pending.rs`), which records nothing, and has
//! the daemon run the login only on Enter: Esc before that is a real
//! cancel, as it is in process. The prompt shown here names this process's
//! device key, and the daemon records its own, which is the same file only
//! when both resolve one key directory (`OLLAMA_DEVICE_KEY_DIR`, else each
//! process's `~/.ollama`). So the daemon's prompt is compared with the one
//! shown, and shown too when it names another key.

use rho_protocol::{LoginId, LoginOutcome, SessionChange};
use rho_providers::{
    auth::{
        browser::{self, BrowserAvailability},
        login_dispatch::{InteractiveLoginMode, ProviderAuthentication},
        login_prompt::LoginPrompt,
    },
    model::{
        catalog::{self, LoginTarget},
        registry,
    },
    provider::{resolve_auth_mode, ProviderAuthKind},
};

use super::{
    daemon_events::LoginUpdate, login::RemovedLogin, login_presentation, App, ComposerMode,
    DefaultTerminal, Entry, PendingLoginComposer, PendingLoginFinish,
};
use crate::credential_store::AppCredentialStore;

/// The interactive login the daemon runs for this TUI.
pub(super) struct DaemonLogin {
    id: LoginId,
    target: LoginTarget,
    prompt: PromptSource,
    /// Esc asked the daemon to cancel it; its end has not arrived yet.
    cancelling: bool,
}

/// Who shows the prompt of a login the daemon runs.
enum PromptSource {
    /// The daemon's prompt is shown when it arrives, under this label.
    Daemon { provider_label: &'static str },
    /// This process showed it, at `url`, before asking the daemon, whose
    /// copy is dropped when it names the same URL: the person already
    /// confirmed that one.
    ShownHere { url: String },
}

impl App {
    /// Has the daemon check and save `key` for `target`, then finishes the
    /// login.
    pub(super) async fn save_daemon_api_key(
        &mut self,
        target: LoginTarget,
        key: String,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.set_status(format!("checking the {} key", target.provider));
        terminal.draw(|frame| self.draw(frame))?;
        let session = self.daemon_session();
        match session.save_key(&target.auth, key).await {
            Ok(saved) => {
                if let Some(unchecked) = saved.unchecked {
                    self.insert_entry(&Entry::Notice(format!(
                        "saved the {} key without checking it: {}",
                        target.provider, unchecked.message
                    )));
                }
                self.finish_daemon_login(target, terminal).await
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not save the {} key: {error:#}",
                    target.provider
                )));
                self.set_status("login failed");
                Ok(())
            }
        }
    }

    /// Starts `target`'s interactive login in the daemon and shows its
    /// prompt. Its end arrives as a view; see [`Self::show_daemon_login`].
    pub(super) async fn start_daemon_login(
        &mut self,
        target: LoginTarget,
        provider_label: &'static str,
        mode: InteractiveLoginMode,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if let Some(login) = &self.daemon_login {
            // A cancelled login is over once the daemon says how it ended:
            // one its provider approved first is still saved.
            self.insert_entry(&Entry::Notice(
                if login.cancelling {
                    "The last login is still being cancelled; try again in a moment."
                } else {
                    "Interactive login is already in progress."
                }
                .into(),
            ));
            return Ok(());
        }
        self.set_status(format!("starting {provider_label} login"));
        terminal.draw(|frame| self.draw(frame))?;
        // The daemon's end maps them back (`app/login/relay_wire.rs`,
        // which only a platform with a daemon builds).
        let flow = match mode {
            InteractiveLoginMode::Browser => rho_protocol::LoginFlow::Browser,
            InteractiveLoginMode::Device => rho_protocol::LoginFlow::DeviceCode,
        };
        let session = self.daemon_session_mut();
        match session.start_login(&target.auth, flow).await {
            Ok((id, first)) => {
                self.daemon_login = Some(DaemonLogin {
                    id,
                    target,
                    prompt: PromptSource::Daemon { provider_label },
                    cancelling: false,
                });
                self.show_daemon_login(first, terminal).await
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not start the {provider_label} login: {error:#}"
                )));
                self.set_status("login failed");
                Ok(())
            }
        }
    }

    /// Enter on a login the provider cannot confirm, whose prompt this
    /// process showed: the daemon runs the login, which records it at
    /// once, and its end finishes the login here
    /// ([`Self::show_daemon_login`]). A refusal keeps the pending login
    /// open, so Enter retries.
    pub(super) async fn record_daemon_login(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let ComposerMode::InteractivePending(pending) = self.input_ui.composer() else {
            return Ok(());
        };
        // A second Enter while the daemon records the first.
        if self.daemon_login.is_some() {
            return Ok(());
        }
        let (target, finish) = (pending.target.clone(), pending.finish.clone());
        let url = pending.prompt.copyable_url().to_string();
        self.set_status(format!("recording the {} login", target.provider));
        terminal.draw(|frame| self.draw(frame))?;
        // The provider has one flow, so the daemon ignores this one.
        let flow = rho_protocol::LoginFlow::default();
        match self
            .daemon_session_mut()
            .start_login(&target.auth, flow)
            .await
        {
            Ok((id, first)) => {
                self.daemon_login = Some(DaemonLogin {
                    id,
                    target,
                    prompt: PromptSource::ShownHere { url },
                    cancelling: false,
                });
                self.show_daemon_login(first, terminal).await
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!("could not record login: {error:#}")));
                self.set_status(format!(
                    "login failed · {}",
                    login_presentation::login_key_hint(&finish)
                ));
                Ok(())
            }
        }
    }

    /// Shows a step of the login under way: its prompt, with the browser
    /// opened here, or its end.
    pub(super) async fn show_daemon_login(
        &mut self,
        update: LoginUpdate,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let Some(login) = &self.daemon_login else {
            return Ok(());
        };
        match update {
            LoginUpdate::Prompt { login: id, prompt } if id == login.id => {
                let provider_label = match &login.prompt {
                    PromptSource::Daemon { provider_label } => *provider_label,
                    PromptSource::ShownHere { url } => {
                        // The daemon recorded the login it prompts for. A
                        // prompt at another URL names a key the person has
                        // not approved, which is the one its sessions sign
                        // with.
                        if *url != prompt.url {
                            self.insert_entry(&Entry::Error(format!(
                                "the rho daemon signs in with a different device key than the one shown above; approve it too: {}",
                                prompt.url
                            )));
                        }
                        return Ok(());
                    }
                };
                let target = login.target.clone();
                let prompt = match prompt.user_code {
                    Some(code) => LoginPrompt::device_code(
                        prompt.url,
                        code,
                        prompt.url_with_code,
                        prompt.instruction,
                    ),
                    None => LoginPrompt::browser_flow(prompt.url, prompt.instruction),
                };
                // The browser opens here, so this process's display decides.
                let opened =
                    browser::try_open(prompt.copyable_url(), BrowserAvailability::from_process());
                let prompt = prompt.with_browser(opened);
                for line in login_presentation::notice_lines(provider_label, &prompt) {
                    self.insert_entry(&Entry::Notice(line));
                }
                let finish = PendingLoginFinish::ProviderCallback;
                self.set_status(format!(
                    "waiting for {provider_label} login · {}",
                    login_presentation::login_key_hint(&finish)
                ));
                self.input_ui.set_composer(ComposerMode::InteractivePending(
                    PendingLoginComposer {
                        target,
                        prompt,
                        finish,
                    },
                ));
                Ok(())
            }
            LoginUpdate::Finished { login: id, outcome } if id == login.id => {
                let target = login.target.clone();
                self.end_daemon_login();
                match outcome {
                    // Also one the provider approved before a cancel
                    // reached the daemon.
                    LoginOutcome::Succeeded => self.finish_daemon_login(target, terminal).await,
                    LoginOutcome::Cancelled => {
                        self.set_status("login cancelled");
                        Ok(())
                    }
                    LoginOutcome::Failed { message } => {
                        self.fail_daemon_login(message);
                        Ok(())
                    }
                    // `Unknown`, or an outcome a newer daemon added.
                    _ => {
                        self.fail_daemon_login(
                            "the login ended in a way this rho does not know".into(),
                        );
                        Ok(())
                    }
                }
            }
            LoginUpdate::Lost => {
                self.end_daemon_login();
                self.fail_daemon_login(
                    "lost the connection to the rho daemon during the login".into(),
                );
                Ok(())
            }
            // Another login's, which this TUI no longer waits for.
            LoginUpdate::Prompt { .. } | LoginUpdate::Finished { .. } => Ok(()),
        }
    }

    /// Esc on the pending login: the daemon is asked to cancel it, and
    /// says how it ended.
    pub(super) fn cancel_daemon_login(&mut self) {
        let Some(login) = &mut self.daemon_login else {
            return;
        };
        login.cancelling = true;
        let id = login.id.clone();
        self.daemon_session().cancel_login(id);
    }

    /// Forgets the login of a session the TUI stops following: closing its
    /// connection cancels it in the daemon.
    pub(super) fn drop_daemon_login(&mut self) {
        self.daemon_login = None;
    }

    /// Whether the daemon runs an interactive login for this TUI.
    pub(super) fn daemon_login_pending(&self) -> bool {
        self.daemon_login.is_some()
    }

    /// Marks the session signed out when the logout of `target` left its
    /// auth without credentials, which the header hints, the statusline,
    /// and a prompt's detour to `/login` follow (`setup_state`). Reports
    /// whether it did.
    fn mark_signed_out_after_logout(&mut self, target: &LoginTarget) -> bool {
        let runtime = &self.info.runtime;
        if runtime.provider != target.provider
            || runtime.auth != target.auth
            || self.session_auth_has_credentials()
        {
            return false;
        }
        self.info.services.auth_unavailable =
            Some(registry::missing_credentials_error(&target.provider).to_string());
        true
    }

    fn session_auth_has_credentials(&self) -> bool {
        ProviderAuthentication::has_credentials(
            self.credential_store.as_ref(),
            &self.info.runtime.auth,
        )
        .unwrap_or(false)
    }

    /// Whether a prompt should be held for `/login` rather than sent: the
    /// session reads as signed out and the store, read again, still has
    /// nothing for its auth. A login made elsewhere since, such as
    /// `rho login` in another terminal, lets the prompt through.
    pub(super) fn daemon_prompt_needs_login(&mut self) -> bool {
        if self.setup_state().signed_in {
            return false;
        }
        AppCredentialStore::reread_external_logins();
        if self.session_auth_has_credentials() {
            self.info.services.auth_unavailable = None;
            return false;
        }
        true
    }

    /// What a logout of `target` removes, as this process reads the store
    /// and the config before the daemon changes them: the daemon's answer
    /// does not say.
    fn login_to_remove(&self, target: &LoginTarget) -> RemovedLogin {
        let forgot_login = rho_providers::provider::provider_descriptor(&target.provider)
            .is_some_and(|descriptor| {
                descriptor.collects_login_endpoint()
                    && self
                        .info
                        .services
                        .config_repository
                        .load()
                        .is_ok_and(|config| !config.providers.awaits_endpoint_login(*descriptor))
            });
        // A keyless profile counts as having credentials, but a logout
        // finds none of its own to delete.
        let keyless = resolve_auth_mode(&target.auth)
            .is_some_and(|(_, mode)| matches!(mode.auth_kind, ProviderAuthKind::None));
        let deleted = !keyless
            && ProviderAuthentication::has_stored_credentials(
                self.credential_store.as_ref(),
                &target.auth,
            )
            .unwrap_or(false);
        RemovedLogin {
            forgot_login,
            deleted,
        }
    }

    /// Has the daemon sign out of `target` and reports it.
    pub(super) async fn sign_out_of_daemon(&mut self, target: &LoginTarget) -> anyhow::Result<()> {
        // The OS store caches reads, misses included, and the daemon may
        // have saved this login since the last one.
        AppCredentialStore::reread_external_logins();
        let removed = self.login_to_remove(target);
        match self.daemon_session().sign_out(&target.auth).await {
            Ok(()) => {
                AppCredentialStore::reread_external_logins();
                self.refresh_available_auths();
                self.insert_entry(&Entry::Notice(removed.notice(target)));
                if self.mark_signed_out_after_logout(target) {
                    self.insert_entry(&Entry::Notice(
                        super::login::ACTIVE_PROVIDER_SIGNED_OUT.into(),
                    ));
                    self.set_status(super::login::NO_PROVIDER_STATUS);
                } else {
                    self.set_status("logout complete");
                }
            }
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not log out of {}: {error:#}",
                    target.provider
                )));
                self.set_status("logout failed");
            }
        }
        Ok(())
    }

    /// What follows a credential the daemon saved for `target`: this
    /// process reads the store again, the provider's model list is
    /// refreshed, and the daemon's sessions offer its models.
    pub(super) async fn finish_daemon_login(
        &mut self,
        target: LoginTarget,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        // The OS store caches reads, misses included.
        AppCredentialStore::reread_external_logins();
        self.persist_default_login_auth(&target);
        self.refresh_available_auths();
        self.refresh_model_list_after_login(&target, terminal)
            .await?;
        self.configure_daemon(SessionChange::ModelCatalog)?;
        if self.finish_decision_host_login(&target).await {
            return Ok(());
        }
        let in_use =
            target.provider == self.info.runtime.provider && target.auth == self.info.runtime.auth;
        // A signed-out session moves to the provider just signed in to, as
        // it does in process. Asked for, the switch counts as made: the
        // daemon builds the provider from the credential it just saved,
        // and says so in the session if it cannot.
        if in_use {
            self.info.services.auth_unavailable = None;
        }
        let switched_to = if in_use || self.setup_state().signed_in {
            None
        } else {
            Some(catalog::default_model_for_provider(&target.provider))
        };
        let status = match switched_to {
            Some(Some(model)) => {
                let reference = rho_providers::provider::model_reference(&target.provider, &model);
                let asked = self.daemon_option_requests.len();
                let model = rho_protocol::ModelRef {
                    provider: target.provider.clone(),
                    model,
                };
                self.configure_daemon(SessionChange::Model {
                    model: model.clone(),
                    auth: Some(target.auth.clone()),
                })?;
                if self.daemon_option_requests.len() > asked {
                    self.info.services.auth_unavailable = None;
                    self.daemon_login_selected = Some(model);
                }
                format!(
                    "stored credentials for {} and selected {reference}",
                    target.provider
                )
            }
            Some(None) => format!(
                "stored credentials for {}, but no cached models are available. Run /refresh-models before switching to it.",
                target.provider
            ),
            None if target.auth == rho_providers::provider::KEYLESS_AUTH => format!(
                "{} is ready. Switch models with /model when you want to use it.",
                target.provider
            ),
            None if in_use => format!(
                "stored credentials for {}; the session uses them from its next prompt",
                target.provider
            ),
            None => format!(
                "stored credentials for {}. Switch models with /model when you want to use it.",
                target.provider
            ),
        };
        self.set_status(status);
        self.announce_held_prompt_after_login();
        self.advance_setup_screen_after_login(terminal);
        self.report_resting_herdr_state().await;
        Ok(())
    }

    fn end_daemon_login(&mut self) {
        self.daemon_login = None;
        self.daemon_session_mut().end_login();
        // Esc already closed it when the person cancelled.
        if matches!(
            self.input_ui.composer(),
            ComposerMode::InteractivePending(_)
        ) {
            self.input_ui.set_composer(ComposerMode::Input);
        }
    }

    fn fail_daemon_login(&mut self, message: String) {
        self.insert_entry(&Entry::Error(message));
        self.set_status("login failed");
    }

    fn daemon_session(&self) -> &super::DaemonSession {
        self.info
            .session
            .daemon
            .as_ref()
            .expect("a daemon login requires a session")
    }

    fn daemon_session_mut(&mut self) -> &mut super::DaemonSession {
        self.info
            .session
            .daemon
            .as_mut()
            .expect("a daemon login requires a session")
    }
}
