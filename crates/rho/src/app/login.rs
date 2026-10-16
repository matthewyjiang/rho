mod secret;

use {
    crate::cli::Command,
    crate::credential_store::AppCredentialStore,
    crate::login_prompt_print::eprint_login_prompt,
    rho_providers::auth::{
        browser::BrowserAvailability,
        login_dispatch::{
            AuthenticationError, AuthenticationMethod, InteractiveLoginCompletion,
            InteractiveLoginMode, ProviderAuthentication,
        },
    },
    rho_providers::model::catalog,
};

/// Both CLI entry points share setup; ACP auth bypasses session/model startup.
pub(super) fn command(command: &Option<Command>) -> Option<(&str, bool)> {
    match command {
        Some(Command::Login {
            provider,
            device_auth,
        }) => Some((provider, *device_auth)),
        Some(Command::Acp(args)) => args.login.as_deref().map(|auth| (auth, false)),
        _ => None,
    }
}

#[derive(Debug)]
pub(super) enum LoginCompletion {
    Saved,
    Unconfirmed,
}

impl LoginCompletion {
    /// Terminal auth's zero exit status is a promise of completed sign-in.
    pub(super) fn finish(self, require_confirmation: bool) -> anyhow::Result<()> {
        match self {
            Self::Unconfirmed if require_confirmation => {
                anyhow::bail!("login could not be confirmed; complete the provider's instructions and reconnect")
            }
            Self::Saved | Self::Unconfirmed => Ok(()),
        }
    }
}

pub(super) async fn run(provider: &str, device_auth: bool) -> anyhow::Result<LoginCompletion> {
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
    match ProviderAuthentication::method(&target.auth)? {
        AuthenticationMethod::None => {
            anyhow::bail!("provider '{provider}' does not require login")
        }
        AuthenticationMethod::ApiKey { entry_label, .. } => {
            let key = secret::read(entry_label)?;
            ProviderAuthentication::save_api_key(&AppCredentialStore, &target.auth, &key)?;
            eprintln!("saved credentials for {}", target.auth);
            return Ok(LoginCompletion::Saved);
        }
        AuthenticationMethod::Interactive { .. } => {}
    }

    let availability = BrowserAvailability::from_process();
    let mode = if device_auth {
        InteractiveLoginMode::Device
    } else {
        ProviderAuthentication::preferred_mode(&target.auth, availability)
    };
    let login = ProviderAuthentication::start_interactive_login_with_availability(
        &target.auth,
        mode,
        availability,
    )
    .await?;
    eprint_login_prompt(login.provider_label, &login.prompt);

    match login.completion {
        InteractiveLoginCompletion::Confirm(completion) => {
            completion.await?.save(&AppCredentialStore)?;
            eprintln!("Successfully logged in to {}", target.auth);
            Ok(LoginCompletion::Saved)
        }
        InteractiveLoginCompletion::Unconfirmed { instruction } => {
            eprintln!("{instruction}");
            Ok(LoginCompletion::Unconfirmed)
        }
    }
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
