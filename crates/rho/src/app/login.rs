//! `rho login` and `rho acp --login`: signing in from a terminal.
//!
//! `rho login` is a client of the rho daemon, which saves the credential
//! and runs interactive logins for it (#1662), starting the daemon when
//! none runs (`daemon_client.rs`). `rho acp --login`, which an ACP host runs
//! in a terminal, signs in the same way. Before the first sign-in, it asks
//! which credential store to keep it in (`store_choice.rs`).

mod daemon_client;
pub(super) mod drive;
pub(super) mod relay_wire;
mod secret;
pub(super) mod store_choice;

use rho_providers::{
    auth::login_dispatch::{AuthenticationError, AuthenticationMethod, ProviderAuthentication},
    model::catalog,
    provider::{provider_descriptor, resolve_auth_mode, ProviderAuthKind},
};

use crate::cli::Command;

/// A sign-in someone asked for from a terminal.
#[derive(Clone, Copy, Debug)]
pub(super) enum LoginCommand<'a> {
    /// `rho login <provider>`, with `--device-auth`.
    Cli {
        provider: &'a str,
        device_auth: bool,
    },
    /// `rho acp --login <auth>`, which an ACP host runs in a terminal and
    /// whose zero exit status the host takes as signed in: an API key the
    /// daemon checked and saved, or a login the provider confirmed. A login
    /// whose approval nothing reports is refused (see
    /// [`confirms_sign_in`]).
    AcpTerminal { auth: &'a str },
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

/// Signs `command` in through the daemon, which saves the credential under
/// its own config.
pub(super) async fn run(command: LoginCommand<'_>) -> anyhow::Result<()> {
    match command {
        LoginCommand::Cli {
            provider,
            device_auth,
        } => daemon_client::run(target(provider)?, device_auth).await,
        LoginCommand::AcpTerminal { auth } => {
            let target = target(auth)?;
            if !confirms_sign_in(&target.auth) {
                anyhow::bail!(
                    "'{auth}' signs in without reporting when the sign-in is approved, so an ACP host cannot run it; run `rho login {auth}` in a terminal instead"
                );
            }
            daemon_client::run(target, /*device_auth*/ false).await
        }
    }
}

/// Whether the daemon's sign-in to auth profile `auth` finishes only once
/// the provider approved it, or a key it checked is saved. An Ollama Cloud
/// device key is saved as soon as the user is asked to approve it at
/// ollama.com, and nothing reports the approval, so an ACP host, which
/// takes a finished sign-in as signed in, does not offer it, and a TUI
/// following a daemon session asks for it only once the user says they
/// approved (`tui/daemon_login.rs`).
pub(crate) fn confirms_sign_in(auth: &str) -> bool {
    !matches!(
        resolve_auth_mode(auth).map(|(_, mode)| mode.auth_kind),
        Some(ProviderAuthKind::OllamaDeviceKey { .. })
    )
}

/// An auth profile that takes a credential.
struct Target {
    /// The auth profile's id, such as `openai-codex`. The daemon looks up
    /// the provider it signs in to itself.
    auth: String,
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
    if provider_descriptor(provider).is_some_and(|descriptor| descriptor.is_keyless()) {
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
        sign_in,
    })
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
