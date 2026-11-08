//! Signing in in this process: `rho acp --login` until the ACP adapter
//! becomes a daemon client (#1670), and `rho login` where the daemon is
//! unsupported (Windows, #1665). Elsewhere `rho login` asks the daemon
//! (`daemon_client.rs`, #1662).

use std::path::Path;

use rho_providers::auth::{
    browser::BrowserAvailability,
    login_dispatch::{InteractiveLoginCompletion, InteractiveLoginMode, ProviderAuthentication},
};

use super::{secret, SignIn, Target};
use crate::{
    app::{config_repository::ConfigRepository, endpoint_login},
    config::Config,
    credential_store::{self, AppCredentialStore},
    login_prompt_print::eprint_login_prompt,
};

/// What a login whose provider sends no confirmation (Ollama Cloud's device
/// key) does once the provider's instructions are shown.
#[derive(Clone, Copy, Debug)]
pub(super) enum Unconfirmed {
    /// Save it and succeed: the person reads the instructions (`rho
    /// login`, which runs here only off unix).
    #[cfg(not(unix))]
    Accept,
    /// Save it but fail, because the exit status promises a finished
    /// sign-in (`rho acp --login`).
    Refuse,
}

/// Signs `target` in, saving under `config`, which `repository` loads from
/// `config_path`.
pub(super) async fn run(
    target: Target,
    device_auth: bool,
    unconfirmed: Unconfirmed,
    repository: &ConfigRepository,
    config: &mut Config,
    config_path: &Path,
) -> anyhow::Result<()> {
    credential_store::initialize_from_config(config, config_path)?;
    let completion = sign_in(&target, device_auth).await?;
    if let Some(descriptor) = rho_providers::provider::provider_descriptor(&target.provider) {
        endpoint_login::record(repository, *descriptor)?;
    }
    match completion {
        Completion::Saved => Ok(()),
        Completion::Unconfirmed => match unconfirmed {
            #[cfg(not(unix))]
            Unconfirmed::Accept => Ok(()),
            Unconfirmed::Refuse => {
                anyhow::bail!("login could not be confirmed; complete the provider's instructions and reconnect")
            }
        },
    }
}

/// How a sign-in that saved its credential ended.
enum Completion {
    Saved,
    /// Saved, but the provider sends no confirmation.
    Unconfirmed,
}

async fn sign_in(target: &Target, device_auth: bool) -> anyhow::Result<Completion> {
    let auth = &target.auth;
    let provider_label = match target.sign_in {
        SignIn::ApiKey { entry_label } => {
            let key = secret::read(entry_label).await?;
            ProviderAuthentication::save_api_key(&AppCredentialStore, auth, &key)?;
            eprintln!("saved credentials for {auth}");
            return Ok(Completion::Saved);
        }
        SignIn::Interactive { provider_label } => provider_label,
    };
    let availability = BrowserAvailability::from_process();
    let mode = if device_auth {
        InteractiveLoginMode::Device
    } else {
        ProviderAuthentication::preferred_mode(auth, availability)
    };
    let login = ProviderAuthentication::start_interactive_login(auth, mode, availability).await?;
    eprint_login_prompt(provider_label, &login.prompt);
    match login.completion {
        InteractiveLoginCompletion::Confirm(completion) => {
            completion.await?.save(&AppCredentialStore)?;
            eprintln!("Successfully logged in to {auth}");
            Ok(Completion::Saved)
        }
        InteractiveLoginCompletion::Unconfirmed {
            instruction,
            authentication,
        } => {
            authentication.accept(&AppCredentialStore)?;
            eprintln!("{instruction}");
            Ok(Completion::Unconfirmed)
        }
    }
}
