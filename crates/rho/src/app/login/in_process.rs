//! Signing in in this process: `rho acp --login` until the ACP adapter
//! becomes a daemon client (#1670), and `rho login` where the daemon is
//! unsupported (Windows, #1665). Elsewhere `rho login` asks the daemon
//! (`daemon_client.rs`, #1662).

use std::path::Path;

use rho_providers::auth::{browser::BrowserAvailability, login_dispatch::ProviderAuthentication};

use super::{
    drive::{self, Saved},
    secret, SignIn, Target,
};
use crate::{
    app::{config_repository::ConfigRepository, endpoint_login},
    config::Config,
    credential_store::{self, AppCredentialStore},
    login_prompt_print::eprint_login_prompt,
};

/// Signs `target` in, with a device-code login when `device_auth` asks for
/// one, saving under `config`, which `repository` loads from
/// `config_path`. The caller decides what a login the provider cannot
/// confirm ([`Saved::Unconfirmed`]) means for its exit status.
pub(super) async fn run(
    target: Target,
    device_auth: bool,
    repository: &ConfigRepository,
    config: &mut Config,
    config_path: &Path,
) -> anyhow::Result<Saved> {
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
            return Ok(Saved::Confirmed);
        }
        SignIn::Interactive { provider_label } => provider_label,
    };
    let availability = BrowserAvailability::from_process();
    let mode = drive::mode(&auth, device_auth, availability);
    let approval = drive::approve(&auth, mode, availability, |prompt| {
        eprint_login_prompt(provider_label, &prompt);
    })
    .await?;
    let saved = approval.save(*descriptor, repository)?;
    match &saved {
        Saved::Confirmed => eprintln!("Successfully logged in to {auth}"),
        Saved::Unconfirmed { instruction } => eprintln!("{instruction}"),
    }
    Ok(saved)
}
