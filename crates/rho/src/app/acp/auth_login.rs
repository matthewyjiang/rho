//! `authenticate` for hosts that draw URL elicitations.
//!
//! With no credential store chosen, Rho first asks where to store
//! credentials as a form (see [`super::auth_store`]). An API key is then
//! typed into Rho's local key page (see [`super::auth_key`]). OAuth starts as
//! `rho login <auth>` starts it: a browser callback where a browser can
//! appear, device-code over SSH or headless. Instead of printing the URL,
//! Rho sends it (and any device code) to the host as a URL-mode
//! `elicitation/create` scoped to the `authenticate` request, so the host
//! shows it in its own sign-in UI and opens it. Rho waits for the provider,
//! saves the credentials, sends `elicitation/complete`, and answers.
//!
//! Declining the elicitation or cancelling `authenticate` drops the login,
//! which also releases a browser login's callback port or the key page's.

use std::{future::Future, path::PathBuf};

use agent_client_protocol::{
    schema::v1::{
        AuthenticateResponse, CompleteElicitationNotification, CreateElicitationRequest,
        ElicitationAction, ElicitationId, ElicitationRequestScope, ElicitationUrlMode, RequestId,
    },
    Error as AcpError,
};
use rho_providers::{
    auth::{
        browser::BrowserAvailability,
        login_dispatch::{InteractiveLoginCompletion, ProviderAuthentication},
        login_prompt::LoginPrompt,
    },
    provider,
};
use tokio::sync::watch;

use super::{
    auth::{AgentLogin, SignIn},
    auth_key::KeyLogin,
    host_features::Forms,
    AcpClientPort,
};
use crate::{
    app::{bootstrap, config_repository::ConfigRepository},
    config::Config,
    credential_store::{self, AppCredentialStore},
};

/// Signs in to auth profile `auth` for the `authenticate` request
/// `request_id`. `config` is the `--config` override the process started
/// with; `forms` is how the host draws the credential store question.
pub(super) async fn authenticate(
    auth: &str,
    sign_in: SignIn,
    forms: Forms,
    config: Option<PathBuf>,
    request_id: RequestId,
    port: &dyn AcpClientPort,
) -> Result<AuthenticateResponse, AcpError> {
    let Some(login) = super::auth::agent_login(auth, sign_in) else {
        return Err(AcpError::invalid_params().data(format!(
            "'{auth}' is not an authenticate method this connection advertised"
        )));
    };
    let key_login = match login {
        AgentLogin::OAuth => None,
        AgentLogin::ApiKey => Some(KeyLogin::prepare(auth)?),
    };
    super::auth_store::choose_if_unset(config.clone(), forms, &request_id, port).await?;
    let repository = ConfigRepository::new(config.clone());
    let config = open_credential_store(config)
        .map_err(|error| AcpError::internal_error().data(format!("{error:#}")))?;
    match key_login {
        None => sign_in_with_oauth(auth, request_id, port).await?,
        Some(key_login) => {
            let descriptor = provider::resolve_auth_mode(auth).map(|(descriptor, _)| descriptor);
            let endpoint = descriptor
                .and_then(|descriptor| config.resolved_provider_endpoint(descriptor.name));
            key_login
                .sign_in(endpoint.as_ref(), &AppCredentialStore, request_id, port)
                .await?;
            if let Some(descriptor) = descriptor {
                crate::app::login::record_login(&repository, *descriptor).map_err(|error| {
                    AcpError::internal_error().data(format!(
                        "saved the key, but could not record the login: {error:#}"
                    ))
                })?;
            }
        }
    }
    Ok(AuthenticateResponse::new())
}

/// Runs OAuth login `auth`, showing its URL in the host.
async fn sign_in_with_oauth(
    auth: &str,
    request_id: RequestId,
    port: &dyn AcpClientPort,
) -> Result<(), AcpError> {
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
        // `agent_login` admits only logins that confirm.
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
        url: login.prompt.copyable_url(),
        message: message(label, &login.prompt),
    };
    show_until_signed_in(port, request_id, shown, signed_in).await
}

/// One login or logout at a time per connection. A new `authenticate` or
/// `logout` supersedes every earlier one, running or still queued, and starts
/// only after the running one is dropped, so a login's callback port is free
/// before the next login binds one and no login saves after a later logout.
/// Hosts may leave a sign-in without sending `$/cancel_request`, which would
/// otherwise hold the port until the provider times out.
pub(super) struct LoginFlight {
    running: tokio::sync::Mutex<()>,
    /// The generation of the newest `run`; older runs end when it moves.
    latest: watch::Sender<u64>,
}

impl Default for LoginFlight {
    fn default() -> Self {
        Self {
            running: tokio::sync::Mutex::new(()),
            latest: watch::channel(0).0,
        }
    }
}

impl LoginFlight {
    pub(super) async fn run<T>(
        &self,
        login: impl Future<Output = Result<T, AcpError>>,
    ) -> Result<T, AcpError> {
        let mut generation = 0;
        self.latest.send_modify(|latest| {
            *latest += 1;
            generation = *latest;
        });
        let mut newer = self.latest.subscribe();
        let mut superseded = std::pin::pin!(async move {
            // A closed channel cannot happen while `self` lives; treat it
            // as superseded rather than wait forever.
            let _ = newer.wait_for(|latest| *latest != generation).await;
        });
        let running = tokio::select! {
            running = self.running.lock() => running,
            () = &mut superseded => return Err(AcpError::request_cancelled()),
        };
        // `select!` drops the losing login before this statement ends, so the
        // next run cannot take `running` while this login still holds a port.
        let result = tokio::select! {
            result = login => result,
            () = &mut superseded => Err(AcpError::request_cancelled()),
        };
        drop(running);
        result
    }
}

/// Activates the credential store as configured now, from the `--config`
/// override `config`, and returns that config, so the store a login chose
/// after launch is the one used. With no store chosen that is the OS store,
/// which sessions read by default.
pub(super) fn open_credential_store(config: Option<PathBuf>) -> anyhow::Result<Config> {
    let repository = ConfigRepository::new(config);
    let config = repository.load()?;
    credential_store::activate_for_login(&config, &bootstrap::absolute_config_path(&repository)?)?;
    Ok(config)
}

/// A started login as the host shows it.
pub(super) struct ShownLogin<'a> {
    pub(super) auth: &'a str,
    pub(super) provider_label: &'a str,
    /// The page the user opens: the provider's sign-in page or Rho's key page.
    pub(super) url: &'a str,
    pub(super) message: String,
}

/// Shows `login` in the host until `signed_in` finishes. A host that
/// declines or dismisses the elicitation abandons the login.
pub(super) async fn show_until_signed_in(
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
            login.url,
        ),
        login.message.clone(),
    ));
    let mut signed_in = std::pin::pin!(signed_in);
    // The host answers once the user chooses to open the URL, which may be
    // after the provider already finished (a code approved on a phone).
    tokio::select! {
        result = &mut signed_in => result?,
        answer = &mut shown => match answer.map_err(|error| unshown(&login, &error))?.action {
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

fn message(label: &str, prompt: &LoginPrompt) -> String {
    match &prompt.user_code {
        Some(code) => format!("Sign in to {label} with code {code}."),
        None => format!("Sign in to {label} in your browser."),
    }
}

/// The host refused to show the sign-in link, so the login cannot finish
/// here; the terminal login still can.
fn unshown(login: &ShownLogin<'_>, error: &AcpError) -> AcpError {
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
