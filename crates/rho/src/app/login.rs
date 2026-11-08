//! `rho login` and `rho acp --login`: signing in from a terminal.
//!
//! On unix `rho login` is a client of the rho daemon, which saves the
//! credential and runs interactive logins for it (#1662), starting the
//! daemon when none runs (`daemon_client.rs`). `rho acp --login` still signs
//! in in this process until the ACP adapter becomes a daemon client
//! (#1670), as does `rho login` where the daemon is unsupported (Windows,
//! #1665) (`in_process.rs`).

#[cfg(unix)]
mod daemon_client;
mod in_process;
mod secret;

use rho_providers::{
    auth::login_dispatch::{AuthenticationError, AuthenticationMethod, ProviderAuthentication},
    model::catalog,
};

use super::config_repository::ConfigRepository;
use crate::{cli::Command, config::Config};

/// A sign-in someone asked for from a terminal.
#[derive(Clone, Copy, Debug)]
pub(super) enum LoginCommand<'a> {
    /// `rho login <provider>`, with `--device-auth`.
    Cli {
        provider: &'a str,
        device_auth: bool,
    },
    /// `rho acp --login <auth>`, which an ACP host runs in a terminal and
    /// whose zero exit status promises a finished sign-in.
    AcpTerminal { auth: &'a str },
}

impl<'a> LoginCommand<'a> {
    /// The provider or auth profile to sign in to, as given.
    pub(super) fn provider(self) -> &'a str {
        match self {
            Self::Cli { provider, .. } => provider,
            Self::AcpTerminal { auth } => auth,
        }
    }

    /// Whether `--device-auth` asked for a device-code login.
    pub(super) fn device_auth(self) -> bool {
        match self {
            Self::Cli { device_auth, .. } => device_auth,
            Self::AcpTerminal { .. } => false,
        }
    }
}

/// The sign-in `command` asks for; `None` for every other command.
pub(super) fn command(command: &Option<Command>) -> Option<LoginCommand<'_>> {
    match command {
        Some(Command::Login {
            provider,
            device_auth,
        }) => Some(LoginCommand::Cli {
            provider,
            device_auth: *device_auth,
        }),
        Some(Command::Acp(args)) => args
            .login
            .as_deref()
            .map(|auth| LoginCommand::AcpTerminal { auth }),
        _ => None,
    }
}

/// Signs `command` in. `config`, loaded from `repository` at `config_path`
/// with its credential store chosen, is what an in-process login saves
/// under; a daemon login uses the daemon's.
pub(super) async fn run(
    command: LoginCommand<'_>,
    repository: &ConfigRepository,
    config: &mut Config,
    config_path: &std::path::Path,
) -> anyhow::Result<()> {
    let target = target(command.provider())?;
    match command {
        #[cfg(unix)]
        LoginCommand::Cli { device_auth, .. } => daemon_client::run(target, device_auth).await,
        #[cfg(not(unix))]
        LoginCommand::Cli { device_auth, .. } => {
            in_process::run(
                target,
                device_auth,
                in_process::Unconfirmed::Accept,
                repository,
                config,
                config_path,
            )
            .await
        }
        LoginCommand::AcpTerminal { .. } => {
            in_process::run(
                target,
                /*device_auth*/ false,
                in_process::Unconfirmed::Refuse,
                repository,
                config,
                config_path,
            )
            .await
        }
    }
}

/// An auth profile that takes a credential.
struct Target {
    /// The auth profile's id, such as `openai-codex`.
    auth: String,
    /// The provider's id, such as `openai`.
    provider: String,
    sign_in: SignIn,
}

/// How a [`Target`] signs in.
#[derive(Clone, Copy)]
enum SignIn {
    /// With an API key, read hidden from the terminal.
    ApiKey { entry_label: &'static str },
    /// With the provider's interactive login.
    Interactive { provider_label: &'static str },
}

/// The auth profile `provider`, a provider or auth profile id, signs in to.
fn target(provider: &str) -> anyhow::Result<Target> {
    if rho_providers::provider::provider_descriptor(provider)
        .is_some_and(|descriptor| descriptor.is_keyless())
    {
        anyhow::bail!("provider '{provider}' does not require login");
    }
    let Some(target) = catalog::login_target_for_provider(provider) else {
        if let Err(error @ AuthenticationError::AmbiguousProvider { .. }) =
            ProviderAuthentication::method(provider)
        {
            anyhow::bail!("{error}");
        }
        let options = catalog::login_targets()
            .into_iter()
            .map(|target| target.auth)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("unsupported login provider '{provider}'. Use one of: {options}");
    };
    let sign_in = match ProviderAuthentication::method(&target.auth)? {
        AuthenticationMethod::None => {
            anyhow::bail!("provider '{provider}' does not require login")
        }
        AuthenticationMethod::ApiKey { entry_label } => SignIn::ApiKey { entry_label },
        AuthenticationMethod::Interactive { provider_label } => {
            SignIn::Interactive { provider_label }
        }
    };
    Ok(Target {
        auth: target.auth,
        provider: target.provider,
        sign_in,
    })
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
