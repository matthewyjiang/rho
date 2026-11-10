//! Signing in in this process: `rho login` where the daemon is unsupported
//! (Windows, #1665). Elsewhere `rho login` asks the daemon
//! (`daemon_client.rs`, #1662).

use std::path::Path;

use rho_providers::auth::{browser::BrowserAvailability, login_dispatch::ProviderAuthentication};

use super::{drive, secret, SignIn, Target};
use crate::{
    app::{config_repository::ConfigRepository, endpoint_login},
    config::Config,
    credential_store::{self, AppCredentialStore},
    login_prompt_print::eprint_login_prompt,
};

/// Signs `target` in, with a device-code login when `device_auth` asks for
/// one, saving under `config`, which `repository` loads from
/// `config_path`.
pub(super) async fn run(
    target: Target,
    device_auth: bool,
    repository: &ConfigRepository,
    config: &mut Config,
    config_path: &Path,
) -> anyhow::Result<()> {
    credential_store::initialize_from_config(config, config_path)?;
    let Target {
        auth,
        descriptor,
        sign_in,
    } = target;
    let provider_label = match sign_in {
        SignIn::ApiKey { entry_label } => {
            let key = secret::read(entry_label).await?;
            ProviderAuthentication::save_api_key(&AppCredentialStore, &auth, &key)?;
            endpoint_login::record(repository, *descriptor)?;
            eprintln!("saved credentials for {auth}");
            return Ok(());
        }
        SignIn::Interactive { provider_label } => provider_label,
    };
    let availability = BrowserAvailability::from_process();
    let mode = drive::mode(&auth, device_auth, availability);
    let approval = drive::approve(&AppCredentialStore, &auth, mode, availability, |prompt| {
        eprint_login_prompt(provider_label, &prompt);
    })
    .await?;
    approval.save(*descriptor, repository, &AppCredentialStore)?;
    // As the daemon's `rho login` reports it: an Ollama Cloud device key
    // saves before the user approves it, and the prompt said to approve it,
    // so this claims the save, not the approval.
    eprintln!("Saved the {auth} credential");
    Ok(())
}
