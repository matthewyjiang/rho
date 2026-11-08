//! Driving a provider's interactive login, for each host that runs one in
//! its own process: the daemon's `rho/auth/login` (#1662) and the
//! in-process `rho login` and `rho acp --login` (`in_process.rs`).
//!
//! [`approve`] runs the provider's flow until the provider approves it;
//! dropping its future cancels the login, which closes a browser login's
//! callback port, so the daemon races it against a cancel. [`Approval::save`]
//! then saves what the provider approved. Each host keeps how it shows the
//! prompt, where it runs the save, and what it reports.

use rho_providers::{
    auth::{
        browser::BrowserAvailability,
        login_dispatch::{
            AuthenticationError, CompletedAuthentication, InteractiveLoginCompletion,
            InteractiveLoginMode, ProviderAuthentication, UnconfirmedAuthentication,
        },
        login_prompt::LoginPrompt,
    },
    provider::ProviderDescriptor,
};

use crate::{
    app::{config_repository::ConfigRepository, endpoint_login},
    credential_store::AppCredentialStore,
};

/// The flow a login to `auth` runs: the device code when `device_auth`
/// asks for it, else the one the provider prefers where a browser is
/// `availability`.
pub(in crate::app) fn mode(
    auth: &str,
    device_auth: bool,
    availability: BrowserAvailability,
) -> InteractiveLoginMode {
    if device_auth {
        InteractiveLoginMode::Device
    } else {
        ProviderAuthentication::preferred_mode(auth, availability)
    }
}

/// Starts `auth`'s login in `mode`, opening the browser only where
/// `availability` allows, hands its prompt to `show`, and waits until the
/// provider approves it. Dropping the future cancels the login.
pub(in crate::app) async fn approve(
    auth: &str,
    mode: InteractiveLoginMode,
    availability: BrowserAvailability,
    show: impl FnOnce(LoginPrompt),
) -> Result<Approval, AuthenticationError> {
    let login = ProviderAuthentication::start_interactive_login(auth, mode, availability).await?;
    show(login.prompt);
    match login.completion {
        InteractiveLoginCompletion::Confirm(completion) => {
            completion.await.map(Approval::Confirmed)
        }
        InteractiveLoginCompletion::Unconfirmed {
            instruction,
            authentication,
        } => Ok(Approval::Unconfirmed {
            instruction,
            authentication,
        }),
    }
}

/// What a provider approved, to save.
pub(in crate::app) enum Approval {
    Confirmed(CompletedAuthentication),
    /// The provider sends no confirmation (Ollama Cloud's device key), so
    /// the login works once the user did what `instruction` says.
    Unconfirmed {
        instruction: &'static str,
        authentication: UnconfirmedAuthentication,
    },
}

/// What [`Approval::save`] saved.
pub(in crate::app) enum Saved {
    /// A login the provider confirmed, or a key the user entered.
    Confirmed,
    /// A login the provider sends no confirmation for, with what the user
    /// must still do.
    Unconfirmed { instruction: &'static str },
}

impl Approval {
    /// Saves it to the credential store in use, and records the login of
    /// `descriptor`'s provider when it is logged in through its endpoint
    /// (Ollama) in `repository`'s config. Blocks on both.
    pub(in crate::app) fn save(
        self,
        descriptor: ProviderDescriptor,
        repository: &ConfigRepository,
    ) -> anyhow::Result<Saved> {
        let saved = match self {
            Self::Confirmed(completed) => {
                completed.save(&AppCredentialStore)?;
                Saved::Confirmed
            }
            Self::Unconfirmed {
                instruction,
                authentication,
            } => {
                authentication.accept(&AppCredentialStore)?;
                Saved::Unconfirmed { instruction }
            }
        };
        endpoint_login::record(repository, descriptor)?;
        Ok(saved)
    }
}
