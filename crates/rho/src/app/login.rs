use {
    crate::credential_store::AppCredentialStore,
    rho_providers::auth::login_dispatch::{
        AuthenticationMethod, InteractiveLoginMode, InteractiveUserAction, ProviderAuthentication,
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
        AuthenticationMethod::ApiKey { entry_label } => {
            anyhow::bail!(
                "{entry_label} login is only supported in the interactive TUI; run `/login {provider}`"
            );
        }
        AuthenticationMethod::Interactive { .. } => {}
    }

    let mode = if device_auth {
        InteractiveLoginMode::Device
    } else {
        InteractiveLoginMode::Browser
    };
    let login = ProviderAuthentication::start_interactive_login(&target.auth, mode).await?;
    match &login.user_action {
        InteractiveUserAction::BrowserOpened => {
            if ProviderAuthentication::supports_device_login(&target.auth) {
                eprintln!(
                    "Opening browser for {} login. On a remote or headless session, use `rho login {} --device-auth` instead.",
                    login.provider_label, target.auth
                );
            } else {
                eprintln!(
                    "Opening browser for {} login. This provider does not support device login; use an API key on a remote or headless session.",
                    login.provider_label
                );
            }
        }
        InteractiveUserAction::OpenUrl { url, instruction } => {
            eprintln!("{}: {instruction}", login.provider_label);
            eprintln!("{url}");
        }
        InteractiveUserAction::DeviceCode {
            verification_uri,
            user_code,
            verification_uri_complete,
        } => {
            eprintln!(
                "{} login: visit {verification_uri} and enter code {user_code}",
                login.provider_label
            );
            if let Some(uri) = verification_uri_complete {
                eprintln!("Or open this URL to continue: {uri}");
            }
        }
    }

    login.completion.await?.save(&AppCredentialStore)?;
    eprintln!("Successfully logged in to {}", target.auth);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rho_providers::auth::login_dispatch::AuthenticationMethod;

    #[test]
    fn ollama_cloud_device_login_target_resolves_interactive_method() {
        let target = catalog::login_target_for_provider("ollama-cloud-device")
            .expect("ollama-cloud-device login target");
        assert_eq!(target.auth, "ollama-cloud-device");
        assert_eq!(
            ProviderAuthentication::method(&target.auth).unwrap(),
            AuthenticationMethod::Interactive {
                provider_label: "Ollama Cloud",
            }
        );
        assert!(ProviderAuthentication::supports_device_login(&target.auth));
    }
}
