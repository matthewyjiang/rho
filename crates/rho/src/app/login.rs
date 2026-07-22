use {
    crate::credential_store::AppCredentialStore,
    rho_providers::auth::login_dispatch::{
        AuthenticationMethod, OAuthMode, OAuthUserAction, ProviderAuthentication,
    },
    rho_providers::model::catalog,
};

pub(super) async fn run(provider: &str, device_auth: bool) -> anyhow::Result<()> {
    if rho_providers::provider::provider_descriptor(provider).is_some_and(|descriptor| {
        descriptor.auth_kind == rho_providers::provider::ProviderAuthKind::None
    }) {
        anyhow::bail!("provider '{provider}' does not require login");
    }
    let Some(target) = catalog::login_target_for_provider(provider) else {
        let providers = catalog::login_targets()
            .into_iter()
            .map(|target| target.provider)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!("unsupported login provider '{provider}'. Use one of: {providers}");
    };
    match ProviderAuthentication::method(&target.provider)? {
        AuthenticationMethod::None => {
            anyhow::bail!("provider '{provider}' does not require login")
        }
        AuthenticationMethod::ApiKey { entry_label } => {
            anyhow::bail!(
                "{entry_label} login is only supported in the interactive TUI; run `/login {provider}`"
            );
        }
        AuthenticationMethod::OAuth { .. } => {}
    }

    let mode = if device_auth {
        OAuthMode::Device
    } else {
        OAuthMode::Browser
    };
    let login = ProviderAuthentication::start_oauth(&target.provider, mode).await?;
    match &login.user_action {
        OAuthUserAction::BrowserOpened => {
            if ProviderAuthentication::supports_device_login(&target.provider) {
                eprintln!(
                    "Opening browser for {} login. On a remote or headless session, use `rho login {} --device-auth` instead.",
                    login.provider_label, target.provider
                );
            } else {
                eprintln!(
                    "Opening browser for {} login. This provider does not support device login; use an API key on a remote or headless session.",
                    login.provider_label
                );
            }
        }
        OAuthUserAction::DeviceCode {
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
    eprintln!("Successfully logged in to {}", target.provider);
    Ok(())
}
