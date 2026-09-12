//! Offline editing of model-scoped instructions. No provider or session starts.

use std::{env, io::IsTerminal, process::Stdio};

use anyhow::{ensure, Context};

use crate::{
    cli::{Cli, ModelPromptCommand},
    external_editor::{editor_command, resolve_editor},
    prompt::model_prompt_edit::{EditOutcome, ModelPromptEdit},
};

pub(super) async fn run(command: &ModelPromptCommand, cli: &Cli) -> anyhow::Result<()> {
    let ModelPromptCommand::Edit { provider, model } = command;
    ensure!(
        provider.is_none() || cli.provider.is_none(),
        "pass --provider either before the command or after edit, not both"
    );
    ensure!(
        model.is_none() || cli.model.is_none(),
        "pass --model either before the command or after edit, not both"
    );
    let config = super::config_repository::ConfigRepository::new(cli.config.clone()).load()?;
    let model_reference = model.as_deref().or(cli.model.as_deref());
    // Nonterminal callers need the model flag even if no editor is configured.
    ensure!(
        model_reference.is_some()
            || (std::io::stdin().is_terminal() && std::io::stdout().is_terminal()),
        "model selection requires an interactive terminal; pass --model <MODEL> to edit directly"
    );
    let editor = resolve_editor(env::var_os("VISUAL"), env::var_os("EDITOR"))
        .context("set VISUAL or EDITOR to edit a model prompt")?;
    let mut command = editor_command(&editor)?;
    let explicit_provider = provider.as_deref().or(cli.provider.as_deref());
    let (provider, model) = if let Some(reference) = model_reference {
        let resolved = config.model_aliases.resolve(reference)?;
        if let (Some(explicit), Some(alias_provider)) =
            (explicit_provider, resolved.provider.as_deref())
        {
            ensure!(canonical_provider(explicit) == canonical_provider(alias_provider),
                "model alias '{reference}' selects provider '{alias_provider}', which conflicts with --provider '{explicit}'");
        }
        let provider = canonical_provider(
            resolved
                .provider
                .as_deref()
                .or(explicit_provider)
                .unwrap_or(&config.provider),
        );
        (provider.to_owned(), resolved.model)
    } else {
        let Some(selected) = crate::tui::model_prompt_picker::select(
            &config,
            explicit_provider.map(canonical_provider),
        )
        .await?
        else {
            return Ok(());
        };
        (selected.provider, selected.model)
    };
    let home = crate::paths::home_dir().context("could not locate the home directory")?;
    let edit = ModelPromptEdit::prepare(&home, &provider, &model)?;
    command
        .arg(edit.path())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    eprintln!("editing model prompt for {provider}/{model}");
    let result: anyhow::Result<()> = async {
        #[cfg(unix)]
        let _signals = crate::external_editor::unix_suspended_child_signals::SuspendedChildSignalGuard::install(&mut command)
            .context("could not prepare editor signal handling")?;
        let status = command.status().await.context("could not start editor")?;
        ensure!(status.success(), "editor exited with {status}");
        Ok(())
    }.await;
    if let Err(error) = result {
        let path = edit
            .recover()
            .context("could not preserve the editor draft")?;
        return Err(error.context(format!("model prompt draft kept at {}", path.display())));
    }
    match edit.finish()? {
        EditOutcome::Saved(path) => {
            println!("saved model prompt: {}", path.display());
            println!("running sessions pick up changes on model switch, /new, or resume");
        }
        EditOutcome::Unchanged => println!("model prompt unchanged"),
    }
    Ok(())
}

fn canonical_provider(provider: &str) -> &str {
    rho_providers::provider::legacy_provider_alias(provider)
        .map_or(provider, |(canonical, _)| canonical)
}
