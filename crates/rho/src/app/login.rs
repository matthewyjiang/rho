use {
    crate::credential_store::AppCredentialStore,
    crate::login_prompt_print::eprint_login_prompt,
    rho_providers::auth::{
        browser::BrowserAvailability,
        login_dispatch::{
            AuthenticationMethod, InteractiveLoginCompletion, InteractiveLoginMode,
            ProviderAuthentication,
        },
    },
    rho_providers::model::catalog,
};

pub(super) async fn run(provider: &str, device_auth: bool) -> anyhow::Result<()> {
    if rho_providers::provider::provider_descriptor(provider)
        .is_some_and(|descriptor| descriptor.is_keyless())
    {
        anyhow::bail!("provider '{provider}' does not require login");
    }
    let Some(target) = catalog::login_target_for_provider(provider) else {
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
            anyhow::bail!(
                "{entry_label} login is only supported in the interactive TUI; run `/login {provider}`"
            );
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
        }
        InteractiveLoginCompletion::Unconfirmed { instruction } => {
            eprintln!("{instruction}");
        }
    }
    Ok(())
}
