//! `/login` and `/logout` against a daemon session (#1662): the daemon
//! writes the credential store its sessions read, so it checks and saves a
//! key, runs a provider's interactive login, and signs out. The TUI keeps
//! what it shows: the pickers, the key prompt, the login's URL, and the
//! browser, which opens here.
//!
//! Unlike in process, a login does not swap the running provider: the
//! daemon's sessions pick the credential up at their next prompt, and a
//! logout of the profile in use shows as that prompt's authentication
//! error. Nor does it make the session's provider and model the config's
//! defaults: only the default provider's auth profile follows a login.
//! A login its provider cannot confirm (Ollama Cloud's device key) is
//! saved by the daemon as soon as it is shown, so it reads as finished
//! before the person approves it.

use rho_protocol::{LoginId, LoginOutcome, SessionChange};
use rho_providers::{
    auth::{
        browser::{self, BrowserAvailability},
        login_dispatch::{InteractiveLoginMode, ProviderAuthentication},
        login_prompt::LoginPrompt,
    },
    model::{catalog::LoginTarget, registry},
};

use super::{
    daemon_events::LoginUpdate, login_presentation, App, ComposerMode, DefaultTerminal, Entry,
    PendingLoginComposer, PendingLoginFinish,
};
use crate::credential_store::AppCredentialStore;

/// The interactive login the daemon runs for this TUI.
pub(super) struct DaemonLogin {
    id: LoginId,
    target: LoginTarget,
    provider_label: &'static str,
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
        if self.daemon_login.is_some() {
            self.insert_entry(&Entry::Notice(
                "Interactive login is already in progress.".into(),
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
                    provider_label,
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
                let (target, provider_label) = (login.target.clone(), login.provider_label);
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
        if let Some(login) = &self.daemon_login {
            self.daemon_session().cancel_login(login.id.clone());
        }
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
    /// whether it did. The daemon builds the provider; this only says what
    /// the next prompt would find.
    fn daemon_session_signed_out(&mut self, target: &LoginTarget) -> bool {
        let runtime = &self.info.runtime;
        if runtime.provider != target.provider
            || runtime.auth != target.auth
            || ProviderAuthentication::has_credentials(self.credential_store.as_ref(), &target.auth)
                .unwrap_or(false)
        {
            return false;
        }
        self.info.services.auth_unavailable =
            Some(registry::missing_credentials_error(&target.provider).to_string());
        true
    }

    /// Has the daemon sign out of `target` and reports it.
    pub(super) async fn sign_out_of_daemon(&mut self, target: &LoginTarget) -> anyhow::Result<()> {
        match self.daemon_session().sign_out(&target.auth).await {
            Ok(()) => {
                AppCredentialStore::reread_external_logins();
                self.refresh_available_auths();
                self.insert_entry(&Entry::Notice(format!("logged out of {}", target.provider)));
                if !self.daemon_session_signed_out(target) {
                    self.set_status("logout complete");
                } else {
                    self.insert_entry(&Entry::Notice(
                        "the active provider no longer has credentials. Run /login or switch with /model."
                            .into(),
                    ));
                    self.set_status("no providers configured; run /login");
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
        if in_use {
            self.info.services.auth_unavailable = None;
        }
        self.set_status(if target.auth == rho_providers::provider::KEYLESS_AUTH {
            format!(
                "{} is ready. Switch models with /model when you want to use it.",
                target.provider
            )
        } else if in_use {
            format!(
                "stored credentials for {}; the session uses them from its next prompt",
                target.provider
            )
        } else {
            format!(
                "stored credentials for {}. Switch models with /model when you want to use it.",
                target.provider
            )
        });
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
