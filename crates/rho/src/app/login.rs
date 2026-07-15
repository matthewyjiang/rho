use crate::{
    auth::{codex_oauth, github_copilot_device, xai_oauth},
    credentials::{
        save_codex_tokens, save_github_copilot_tokens, save_xai_tokens, OsCredentialStore,
    },
    model::catalog,
    provider::{self, ProviderAuthKind},
};

pub(super) async fn run(provider: &str, device_auth: bool) -> anyhow::Result<()> {
    let Some(target) = catalog::login_target_for_provider(provider) else {
        anyhow::bail!(
            "unsupported login provider '{provider}'. Use one of: {}",
            catalog::implemented_providers().join(", ")
        );
    };
    let Some(descriptor) = provider::provider_descriptor(&target.provider) else {
        anyhow::bail!("unsupported login provider '{}'", target.provider);
    };
    let store = OsCredentialStore;

    match descriptor.auth_kind {
        ProviderAuthKind::CodexOAuth { .. } => {
            let tokens = if device_auth {
                let login = codex_oauth::start_codex_device_login().await?;
                eprintln!(
                    "Codex login: visit {} and enter code {}",
                    login.verification_uri, login.user_code
                );
                codex_oauth::complete_codex_device_login(login).await?
            } else {
                eprintln!("Opening browser for Codex login. On a remote or headless session, use `rho login openai-codex --device-auth` instead.");
                codex_oauth::run_codex_oauth_flow().await?
            };
            save_codex_tokens(&store, &tokens)?;
        }
        ProviderAuthKind::GithubCopilotDevice { .. } => {
            let login = github_copilot_device::start_github_copilot_device_login().await?;
            eprintln!(
                "GitHub Copilot login: visit {} and enter code {}",
                login.verification_uri, login.user_code
            );
            if let Some(uri) = &login.verification_uri_complete {
                eprintln!("Or open this URL to continue: {uri}");
            }
            let tokens = github_copilot_device::complete_github_copilot_device_login(login).await?;
            save_github_copilot_tokens(&store, &tokens)?;
        }
        ProviderAuthKind::XaiOAuth { .. } => {
            let tokens = if device_auth {
                let login = xai_oauth::start_xai_device_login().await?;
                eprintln!(
                    "xAI login: visit {} and enter code {}",
                    login.verification_uri, login.user_code
                );
                if let Some(uri) = &login.verification_uri_complete {
                    eprintln!("Or open this URL to continue: {uri}");
                }
                xai_oauth::complete_xai_device_login(login).await?
            } else {
                eprintln!("Opening browser for xAI login. On a remote or headless session, use `rho login xai --device-auth` instead.");
                xai_oauth::run_xai_oauth_flow().await?
            };
            save_xai_tokens(&store, &tokens)?;
        }
        ProviderAuthKind::ApiKey { entry_label, .. } => {
            anyhow::bail!(
                "{entry_label} login is only supported in the interactive TUI; run `/login {provider}`"
            );
        }
    }

    eprintln!("Successfully logged in to {}", target.provider);
    Ok(())
}
