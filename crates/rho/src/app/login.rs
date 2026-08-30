use {
    crate::credential_store::AppCredentialStore,
    rho_providers::auth::{
        browser::{BrowserAvailability, BrowserEnvironment},
        login_dispatch::{
            AuthenticationMethod, InteractiveLoginCompletion, InteractiveLoginMode,
            ProviderAuthentication,
        },
        login_prompt::LoginPrompt,
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

    let availability = BrowserAvailability::resolve(BrowserEnvironment::from_process());
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
    print_login_prompt(login.provider_label, &login.prompt);

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

fn print_login_prompt(provider_label: &str, prompt: &LoginPrompt) {
    eprintln!("{provider_label} login");
    eprintln!("{}", prompt.url);
    if let Some(code) = &prompt.user_code {
        eprintln!("code: {code}");
    }
    if let Some(complete) = &prompt.url_with_code {
        if complete != prompt.url.as_str() {
            eprintln!("{complete}");
        }
    }
    eprintln!("{}", prompt.browser.note());
    eprintln!("{}", prompt.instruction);
}
