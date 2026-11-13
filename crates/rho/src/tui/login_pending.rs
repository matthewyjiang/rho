//! Pending interactive (browser or device-code) login: start it, let the user
//! confirm or cancel it, and apply the provider's result.

use super::{
    background_tasks::{SessionOutput, TaskId},
    *,
};
use rho_providers::auth::{
    browser::BrowserAvailability,
    login_dispatch::{
        CompletedAuthentication, InteractiveLoginCompletion, InteractiveLoginMode,
        ProviderAuthentication,
    },
};

/// A finished interactive login task and the target it signed in to.
pub(super) struct FinishedInteractiveLogin {
    target: LoginTarget,
    result: Result<Result<CompletedAuthentication, String>, tokio::task::JoinError>,
}

impl App {
    pub(super) async fn start_interactive_login_flow(
        &mut self,
        target: LoginTarget,
        provider_label: &'static str,
        mode: InteractiveLoginMode,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        // A login its provider cannot confirm is shown from here instead,
        // so the daemon, which records one as soon as it runs, is asked
        // only on Enter (`record_daemon_login`).
        if crate::app::confirms_sign_in(&target.auth) {
            return self
                .start_daemon_login(target, provider_label, mode, terminal)
                .await;
        }
        if self.interactive_login_pending() {
            self.insert_entry(&Entry::Notice(
                "Interactive login is already in progress. Press esc to cancel.".into(),
            ));
            return Ok(());
        }

        let availability = BrowserAvailability::from_process();
        self.set_status(match mode {
            InteractiveLoginMode::Browser => format!("starting {provider_label} login"),
            InteractiveLoginMode::Device => format!("starting {provider_label} device login"),
        });
        terminal.draw(|frame| self.draw(frame))?;
        let login = match ProviderAuthentication::start_interactive_login(
            self.credential_store.as_ref(),
            &target.auth,
            mode,
            availability,
        )
        .await
        {
            Ok(login) => login,
            Err(err) => {
                self.insert_entry(&Entry::Error(err.to_string()));
                self.set_status("login failed");
                return Ok(());
            }
        };

        let provider_label = login.provider_label;
        let device_flow = login.prompt.user_code.is_some();
        for line in super::login_presentation::notice_lines(provider_label, &login.prompt) {
            self.insert_entry(&Entry::Notice(line));
        }
        let completion = match login.completion {
            InteractiveLoginCompletion::Confirm(completion) => completion,
            InteractiveLoginCompletion::Unconfirmed {
                instruction,
                authentication,
            } => {
                // Nothing is recorded until Enter, so Esc stays a real cancel.
                let finish = PendingLoginFinish::UserConfirms(authentication);
                self.insert_entry(&Entry::Notice(instruction.into()));
                self.set_status(format!(
                    "waiting for {provider_label} approval · {}",
                    super::login_presentation::login_key_hint(&finish)
                ));
                self.input_ui.set_composer(ComposerMode::InteractivePending(
                    PendingLoginComposer {
                        target,
                        prompt: login.prompt,
                        finish,
                    },
                ));
                self.report_resting_herdr_state().await;
                return Ok(());
            }
        };
        let finish = PendingLoginFinish::ProviderCallback;
        let flow = if device_flow { " device" } else { "" };
        self.set_status(format!(
            "waiting for {provider_label}{flow} login · {}",
            super::login_presentation::login_key_hint(&finish)
        ));
        self.input_ui
            .set_composer(ComposerMode::InteractivePending(PendingLoginComposer {
                target: target.clone(),
                prompt: login.prompt,
                finish,
            }));
        self.tasks.spawn(
            TaskId::InteractiveLogin,
            async move { completion.await.map_err(|err| err.to_string()) },
            move |result| {
                SessionOutput::InteractiveLogin(Box::new(FinishedInteractiveLogin {
                    target,
                    result,
                }))
                .into()
            },
        );
        Ok(())
    }

    pub(super) fn interactive_login_pending(&self) -> bool {
        self.daemon_login_pending() || self.tasks.contains(|id| *id == TaskId::InteractiveLogin)
    }

    /// Esc on the pending-login composer. Drops the task so its result never
    /// applies.
    pub(super) fn abort_interactive_login(&mut self) {
        self.cancel_daemon_login();
        self.tasks.abort(|id| *id == TaskId::InteractiveLogin);
    }

    pub(super) async fn apply_interactive_login(
        &mut self,
        finished: FinishedInteractiveLogin,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let FinishedInteractiveLogin { target, result } = finished;
        match result {
            Ok(Ok(result)) => {
                self.cancel_limits_command().await;
                self.cancel_doctor_command().await;
                let saved = result.save(self.credential_store.as_ref());
                match saved {
                    Ok(()) => {
                        self.input_ui.set_composer(ComposerMode::Input);
                        self.finish_daemon_login(target, terminal).await
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
}
