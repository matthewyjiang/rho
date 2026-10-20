//! `authenticate` for hosts that draw URL elicitations.
//!
//! The login starts as `rho login <auth>` starts it: a browser callback where
//! a browser can appear, device-code over SSH or headless. Instead of printing
//! the URL, Rho sends it (and any device code) to the host as a URL-mode
//! `elicitation/create` scoped to the `authenticate` request, so the host
//! shows it in its own sign-in UI and opens it. Rho waits for the provider,
//! saves the credentials, sends `elicitation/complete`, and answers.
//!
//! Declining the elicitation or cancelling `authenticate` drops the login,
//! which also releases a browser login's callback port.

use std::{future::Future, path::PathBuf, sync::PoisonError};

use agent_client_protocol::{
    schema::v1::{
        AuthenticateResponse, CompleteElicitationNotification, CreateElicitationRequest,
        ElicitationAction, ElicitationId, ElicitationRequestScope, ElicitationUrlMode, RequestId,
    },
    Error as AcpError,
};
use rho_providers::auth::{
    browser::BrowserAvailability,
    login_dispatch::{InteractiveLoginCompletion, ProviderAuthentication},
    login_prompt::LoginPrompt,
};
use tokio::sync::oneshot;

use super::{auth::OAuthSignIn, AcpClientPort};
use crate::{
    app::{bootstrap, config_repository::ConfigRepository},
    credential_store::{self, AppCredentialStore},
};

/// Signs in to auth profile `auth` for the `authenticate` request
/// `request_id`. `config` is the `--config` override the process started with.
pub(super) async fn authenticate(
    auth: &str,
    sign_in: OAuthSignIn,
    config: Option<PathBuf>,
    request_id: RequestId,
    port: &dyn AcpClientPort,
) -> Result<AuthenticateResponse, AcpError> {
    if !super::auth::runs_in_process(auth, sign_in) {
        return Err(AcpError::invalid_params().data(format!(
            "'{auth}' is not an authenticate method this connection advertised"
        )));
    }
    open_credential_store(config)
        .map_err(|error| AcpError::internal_error().data(format!("{error:#}")))?;
    // The host opens the URL, so nothing is launched here; availability
    // only picks the grant, as it does for `rho login`.
    let mode = ProviderAuthentication::preferred_mode(auth, BrowserAvailability::from_process());
    let login = ProviderAuthentication::start_interactive_login_with_availability(
        auth,
        mode,
        BrowserAvailability::Headless,
    )
    .await
    .map_err(|error| sign_in_error(auth, &error))?;
    let label = login.provider_label;
    let completion = match login.completion {
        InteractiveLoginCompletion::Confirm(completion) => completion,
        // `runs_in_process` admits only logins that confirm.
        InteractiveLoginCompletion::Unconfirmed { .. } => {
            return Err(
                AcpError::internal_error().data(format!("{label} sign-in cannot be confirmed"))
            );
        }
    };
    let signed_in = async move {
        completion
            .await
            .map_err(|error| sign_in_error(label, &error))?
            .save(&AppCredentialStore)
            .map_err(|error| {
                AcpError::internal_error()
                    .data(format!("could not save {label} credentials: {error}"))
            })
    };
    let shown = ShownLogin {
        auth,
        provider_label: label,
        prompt: &login.prompt,
    };
    show_until_signed_in(port, request_id, shown, signed_in).await?;
    Ok(AuthenticateResponse::new())
}

/// One login at a time per connection. A new `authenticate` cancels the one
/// before it and waits until that login is dropped, so its callback port is
/// free before the new login binds one. Hosts may leave a sign-in without
/// sending `$/cancel_request`, which would otherwise hold the port until the
/// provider times out.
#[derive(Default)]
pub(super) struct LoginFlight {
    running: tokio::sync::Mutex<()>,
    cancel_running: std::sync::Mutex<Option<oneshot::Sender<()>>>,
}

impl LoginFlight {
    pub(super) async fn run<T>(
        &self,
        login: impl Future<Output = Result<T, AcpError>>,
    ) -> Result<T, AcpError> {
        if let Some(cancel) = self.cancel_slot().take() {
            let _ = cancel.send(());
        }
        let _running = self.running.lock().await;
        let (cancel, superseded) = oneshot::channel();
        *self.cancel_slot() = Some(cancel);
        // The losing branch, and with it the login, drops here, before
        // `_running` lets the next login start.
        tokio::select! {
            result = login => result,
            _ = superseded => Err(AcpError::request_cancelled()),
        }
    }

    fn cancel_slot(&self) -> std::sync::MutexGuard<'_, Option<oneshot::Sender<()>>> {
        self.cancel_running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Activates the configured credential store so the login can save to it.
/// [`super::auth::methods`] advertises no `authenticate` method while the
/// store is unchosen; this refuses if that changed since.
fn open_credential_store(config: Option<PathBuf>) -> anyhow::Result<()> {
    let repository = ConfigRepository::new(config);
    let config = repository.load()?;
    if credential_store::needs_explicit_choice(&config) {
        anyhow::bail!(
            "credential store is unset; choose one with `rho credential-store set os|file`"
        );
    }
    credential_store::activate_for_login(&config, &bootstrap::absolute_config_path(&repository)?)
}

/// A started login as the host shows it.
#[derive(Clone, Copy)]
struct ShownLogin<'a> {
    auth: &'a str,
    provider_label: &'a str,
    prompt: &'a LoginPrompt,
}

/// Shows `login` in the host until `signed_in` finishes. A host that
/// declines or dismisses the elicitation abandons the login.
async fn show_until_signed_in(
    port: &dyn AcpClientPort,
    request_id: RequestId,
    login: ShownLogin<'_>,
    signed_in: impl Future<Output = Result<(), AcpError>>,
) -> Result<(), AcpError> {
    let elicitation_id = ElicitationId::new(uuid::Uuid::new_v4().to_string());
    let mut shown = port.create_elicitation(CreateElicitationRequest::new(
        ElicitationUrlMode::new(
            ElicitationRequestScope::new(request_id),
            elicitation_id.clone(),
            login.prompt.copyable_url(),
        ),
        message(login),
    ));
    let mut signed_in = std::pin::pin!(signed_in);
    // The host answers once the user chooses to open the URL, which may be
    // after the provider already finished (a code approved on a phone).
    tokio::select! {
        result = &mut signed_in => result?,
        answer = &mut shown => match answer.map_err(|error| unshown(login, &error))?.action {
            ElicitationAction::Accept(_) => signed_in.await?,
            ElicitationAction::Decline | ElicitationAction::Cancel => {
                return Err(AcpError::request_cancelled());
            }
            // An action this version does not know is not consent to wait.
            _ => return Err(AcpError::request_cancelled()),
        },
    }
    if let Err(error) = port
        .complete_elicitation(CompleteElicitationNotification::new(elicitation_id))
        .await
    {
        tracing::warn!(%error, "could not mark the sign-in elicitation complete");
    }
    // If the host never answered, dropping the request now withdraws it with
    // `$/cancel_request`; `elicitation/complete` went first, so the host
    // already knows the sign-in finished.
    drop(shown);
    Ok(())
}

fn message(login: ShownLogin<'_>) -> String {
    let label = login.provider_label;
    match &login.prompt.user_code {
        Some(code) => format!("Sign in to {label} with code {code}."),
        None => format!("Sign in to {label} in your browser."),
    }
}

/// The host refused to show the sign-in link, so the login cannot finish
/// here; the terminal login still can.
fn unshown(login: ShownLogin<'_>, error: &AcpError) -> AcpError {
    AcpError::internal_error().data(format!(
        "could not show the {} sign-in link: {error}; run `rho login {}` in a terminal instead",
        login.provider_label, login.auth
    ))
}

fn sign_in_error(subject: &str, error: &impl std::fmt::Display) -> AcpError {
    AcpError::internal_error().data(format!("could not sign in to {subject}: {error}"))
}

#[cfg(test)]
#[path = "auth_login_tests.rs"]
mod tests;
