use std::{
    io::{self, IsTerminal},
    sync::Arc,
};

use crate::{
    cli::{Cli, Command},
    credentials::OsCredentialStore,
    diagnostics::RuntimeDiagnostics,
    herdr::HerdrReporter,
    model::{models_dev::cached_model_metadata, ModelError},
    update,
};

use super::{
    automation, cli_config, config_repository::ConfigRepository, interactive, login,
    sdk_config::SdkBootstrapOptions,
};

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    cli_config::validate(&cli)?;
    if matches!(cli.command, Some(Command::Update)) {
        return update::run_update(env!("CARGO_PKG_VERSION")).await;
    }
    if let Some(Command::Login {
        provider,
        device_auth,
    }) = &cli.command
    {
        return login::run(provider, *device_auth).await;
    }

    let config_path = cli.config.clone();
    let config_repository = ConfigRepository::new(config_path.clone());
    let mut config = config_repository.load()?;
    let store = OsCredentialStore;
    cli_config::refresh_model_cache(&cli, &store).await?;
    if cli_config::apply_overrides(&mut config, &cli)? {
        config_repository.save(&config)?;
    }

    validate_terminal_mode(&cli)?;
    let automation_prompt = automation::prompt_for_command(&cli.command)?;
    if automation_prompt.is_some()
        && config.provider == "anthropic"
        && cached_model_metadata(&config.provider, &config.model).is_none()
    {
        let _ =
            crate::model::models_dev::fetch_model_metadata(&config.provider, &config.model).await;
    }
    let cwd = std::env::current_dir()?;
    let herdr = HerdrReporter::from_env();
    if let Some(prompt) = automation_prompt {
        let (preset_name, output_file) = match &cli.command {
            Some(Command::Run {
                preset,
                output_file,
                ..
            }) => (preset.clone(), output_file.clone()),
            _ => (None, None),
        };
        let preset = preset_name
            .as_deref()
            .map(|name| crate::subagent::find(&cwd, name))
            .transpose()?;
        if let Some(preset) = &preset {
            // Preset overrides apply to this run only; never persist them.
            if let Some(model) = &preset.model {
                config.model = model.clone();
            }
            if let Some(provider) = &preset.provider {
                config.provider = provider.clone();
            }
            if let Some(reasoning) = preset.reasoning {
                config.reasoning = reasoning;
            }
        }
        let diagnostics = RuntimeDiagnostics::new(&config);
        return automation::run(
            prompt,
            automation::Startup {
                config: &config,
                cwd,
                no_system_prompt: cli.no_system_prompt,
                no_tools: cli.no_tools,
                no_subagents: cli.no_subagents,
                preset,
                output_file,
                diagnostics,
                herdr,
            },
        )
        .await;
    }
    let diagnostics = RuntimeDiagnostics::new(&config);

    let pending_update_notice = config
        .check_for_updates
        .then(|| tokio::spawn(update::update_notice(env!("CARGO_PKG_VERSION"))));

    let sdk_options = SdkBootstrapOptions::from_config(&config, &cwd)?;
    let credentials = crate::auth::provider_credentials::ApplicationCredentialSource::new(
        Arc::new(OsCredentialStore),
    );
    let provider_result =
        crate::providers::build_sdk_provider_with_source(sdk_options.provider, &credentials);
    let (missing_auth_error, missing_auth_model_error) = match provider_result {
        Ok(_) => (None, None),
        Err(error) if is_interactive_startup_unavailable_error(&error) => {
            (Some(error.to_string()), Some(error))
        }
        Err(error) => return Err(error.into()),
    };
    let result = interactive::run(interactive::Startup {
        cli: &cli,
        config,
        config_repository,
        cwd,
        missing_auth_error,
        missing_auth_model_error,
        pending_update_notice,
        diagnostics,
        herdr,
    })
    .await;
    result
}

fn validate_terminal_mode(cli: &Cli) -> anyhow::Result<()> {
    if cli.command.is_none() && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
        anyhow::bail!(
            "rho's default mode is the interactive TUI; use `rho run` for non-interactive automation"
        );
    }
    Ok(())
}

fn is_interactive_startup_unavailable_error(error: &ModelError) -> bool {
    matches!(
        error,
        ModelError::MissingApiKey
            | ModelError::MissingCodexAuth
            | ModelError::MissingAnthropicApiKey
            | ModelError::MissingGithubCopilotAuth
            | ModelError::MissingXaiAuth
            | ModelError::Credentials(_)
            | ModelError::UnsupportedProvider(_)
    )
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
