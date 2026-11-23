//! `rho daemon serve` and `rho daemon token`: the CLI over the HTTP
//! WebSocket bridge and its bearer tokens.

use anyhow::Context;
use rho_protocol::ClientEnvironment;

use super::{
    server::{serve_http, HttpOptions},
    tokens::DaemonTokenSet,
};
use crate::{
    app::{
        config_repository::ConfigRepository, http_upgrade, mcp_server::token_store,
        shutdown_signal::ShutdownSignals,
    },
    cli::{Cli, DaemonServeArgs, DaemonTokenCommand},
    credential_store::UntrackedCredentialStore,
    daemon::Executable,
};

/// `rho daemon serve --http`: exposes the local daemon over HTTP WebSockets.
/// Stops on SIGINT or SIGTERM (Ctrl+C on Windows), never on the daemon's own
/// stop requests, so stopping either leaves the other running.
pub(crate) async fn serve(args: &DaemonServeArgs, cli: &Cli) -> anyhow::Result<()> {
    // Taken first, before anything this process starts could inherit it.
    let inherited_listener = args
        .listener_fd
        .map(|fd| {
            http_upgrade::inherit_listener(fd)
                .with_context(|| format!("could not take over the listening socket {fd}"))
        })
        .transpose()?;
    ConfigRepository::new(cli.config.clone()).open_credential_store()?;
    let mut signals = ShutdownSignals::install()?;
    let upgrade = Executable::of_this_process()
        .inspect_err(|error| {
            eprintln!("rho daemon serve: could not find the server's executable; it will not upgrade in place: {error}");
        })
        .ok()
        .and_then(|executable| {
            http_upgrade::Upgrade::new(
                "rho daemon serve",
                executable,
                std::env::args_os().skip(1),
            )
        });
    serve_http(
        HttpOptions {
            address: args.http,
            allowed_hosts: args.allow_host.clone(),
            allowed_origins: args.allow_origin.clone(),
            inherited_listener,
            upgrade,
        },
        ClientEnvironment::of_this_process(),
        async move {
            match signals.recv().await {
                Ok(signal) => eprintln!("rho daemon serve: stopping on {signal}"),
                Err(error) => {
                    eprintln!("rho daemon serve: stopping; could not wait for a signal: {error}")
                }
            }
        },
    )
    .await
}

/// `rho daemon token`: manages bearer tokens for `rho daemon serve --http`.
pub(crate) fn token(command: &DaemonTokenCommand, cli: &Cli) -> anyhow::Result<()> {
    ConfigRepository::new(cli.config.clone()).open_credential_store()?;
    match command {
        DaemonTokenCommand::Rotate { name } => {
            let secret = token_store::update(|tokens: &mut DaemonTokenSet| tokens.rotate(name))?;
            // The secret alone on stdout, so a script can capture it.
            println!("{secret}");
            eprintln!(
                "saved daemon token {name:?}; this is the only time its secret is shown. Send it as `Authorization: Bearer <secret>`."
            );
        }
        DaemonTokenCommand::List { json } => {
            let tokens = DaemonTokenSet::load(&UntrackedCredentialStore)?;
            if *json {
                let names: Vec<_> = tokens
                    .tokens
                    .iter()
                    .map(|token| serde_json::json!({ "name": token.name }))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&names)?);
            } else if tokens.tokens.is_empty() {
                println!("no daemon HTTP tokens; create one with `rho daemon token rotate`");
            } else {
                for token in &tokens.tokens {
                    println!("{}", token.name);
                }
            }
        }
        DaemonTokenCommand::Revoke { name } => {
            token_store::update(|tokens: &mut DaemonTokenSet| {
                if !tokens.revoke(name) {
                    anyhow::bail!("no daemon HTTP token named {name:?}");
                }
                Ok(())
            })?;
            println!("revoked daemon token {name:?}");
        }
    }
    Ok(())
}
