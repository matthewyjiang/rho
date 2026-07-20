use {
    rho_providers::auth::login_dispatch::{
        AuthenticationMethod, OAuthMode, OAuthUserAction, ProviderAuthentication,
    },
    rho_providers::credentials::OsCredentialStore,
    rho_providers::model::catalog,
};

pub(super) async fn run(provider: &str, device_auth: bool) -> anyhow::Result<()> {
    let Some(target) = catalog::login_target_for_provider(provider) else {
        anyhow::bail!(
            "unsupported login provider '{provider}'. Use one of: {}",
            catalog::implemented_providers().join(", ")
        );
    };
    if let AuthenticationMethod::ApiKey { entry_label } =
        ProviderAuthentication::method(&target.provider)?
    {
        anyhow::bail!(
            "{entry_label} login is only supported in the interactive TUI; run `/login {provider}`"
        );
    }

    let mode = if device_auth {
        OAuthMode::Device
    } else {
        OAuthMode::Browser
    };
    let login = ProviderAuthentication::start_oauth(&target.provider, mode).await?;
    match &login.user_action {
        OAuthUserAction::BrowserOpened => {
            eprintln!(
                "Opening browser for {} login. On a remote or headless session, use `rho login {} --device-auth` instead.",
                login.provider_label, target.provider
            );
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

    login.completion.await?.save(&OsCredentialStore)?;
    eprintln!("Successfully logged in to {}", target.provider);
    Ok(())
}
