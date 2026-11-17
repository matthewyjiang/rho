//! `rho mcp serve` and `rho mcp token`: the boundary that reads the process
//! environment (the caller context it started in, the environment sessions
//! run with, the host name the server goes by, the rho home an HTTP
//! server's watches use) and the credential store, then hands resolved
//! values to `app::mcp_server` and `app::session_control`.

use std::sync::Arc;

use rho_protocol::ClientEnvironment;

use crate::{
    app::{
        config_repository::ConfigRepository,
        mcp_server::{
            self, token_store,
            tokens::{Rotation, TokenSet},
            watch, HttpOptions,
        },
        session_control::{Limits, Reach, SessionControl},
    },
    caller_context::{Access, CallerContext},
    cli::{Cli, McpServeArgs, McpTokenCommand, McpTokenScope},
    credential_store::UntrackedCredentialStore,
};

pub(in crate::app) async fn serve(args: &McpServeArgs, cli: &Cli) -> anyhow::Result<()> {
    let defaults = Limits::default();
    let limits = Limits {
        max_depth: args.max_depth.unwrap_or(defaults.max_depth),
        max_sessions: args.max_sessions.unwrap_or(defaults.max_sessions),
    };
    let control = Arc::new(SessionControl::new(limits, Reach::StartIfNeeded));
    let environment = ClientEnvironment::of_this_process();
    let name = match &args.name {
        Some(name) if name.trim().is_empty() => anyhow::bail!("--name must not be empty"),
        Some(name) => name.clone(),
        None => gethostname::gethostname().to_string_lossy().into_owned(),
    };
    match args.http {
        None => {
            let context = CallerContext::from_env(|name| std::env::var_os(name))?;
            let access = args.scope.map_or(Access::Write, access);
            // The remotes live in the credential store. A host whose store
            // cannot open still drives this machine's sessions.
            if let Err(error) = open_credential_store(cli) {
                tracing::warn!(error = %format!("{error:#}"), "could not open the credential store; serving this machine only");
            }
            mcp_server::serve_stdio(control, access, context, environment, name).await
        }
        Some(address) => {
            // The tokens, webhooks, and remotes live in the credential store.
            open_credential_store(cli)?;
            mcp_server::serve_http(
                control,
                HttpOptions {
                    address,
                    allowed_hosts: args.allow_host.clone(),
                    allowed_origins: args.allow_origin.clone(),
                    name,
                    max_watches: args
                        .max_watches
                        .unwrap_or_else(|| watch::default_max_watches(limits.max_sessions)),
                    watches_dir: crate::paths::rho_dir()?.join(watch::DIR),
                },
                environment,
            )
            .await
        }
    }
}

pub(in crate::app) fn token(command: &McpTokenCommand, cli: &Cli) -> anyhow::Result<()> {
    open_credential_store(cli)?;
    match command {
        McpTokenCommand::Rotate {
            name,
            scope,
            workspace,
        } => {
            let secret = token_store::update(|tokens: &mut TokenSet| {
                tokens.rotate(
                    name,
                    Rotation {
                        access: scope.map(access),
                        workspaces: workspace.clone(),
                    },
                )
            })?;
            // The secret alone on stdout, so a script can capture it.
            println!("{secret}");
            eprintln!(
                    "saved token {name:?}; this is the only time its secret is shown. Send it as `Authorization: Bearer <secret>`."
                );
        }
        McpTokenCommand::List { json } => {
            let tokens = TokenSet::load(&UntrackedCredentialStore)?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&token_list(&tokens))?);
            } else if tokens.tokens.is_empty() {
                println!("no MCP server tokens; create one with `rho mcp token rotate`");
            } else {
                for token in &tokens.tokens {
                    let workspaces: Vec<String> = token
                        .workspaces
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect();
                    println!(
                        "{}  {}  {}",
                        token.name,
                        token.access.name(),
                        workspaces.join(", ")
                    );
                }
            }
        }
        McpTokenCommand::Revoke { name } => {
            token_store::update(|tokens: &mut TokenSet| {
                if !tokens.revoke(name) {
                    anyhow::bail!("no MCP server token named {name:?}");
                }
                Ok(())
            })?;
            println!("revoked token {name:?}");
        }
    }
    Ok(())
}

fn access(scope: McpTokenScope) -> Access {
    match scope {
        McpTokenScope::Read => Access::Read,
        McpTokenScope::Write => Access::Write,
    }
}

/// `rho mcp token list --json`: every token without its hash.
fn token_list(tokens: &TokenSet) -> serde_json::Value {
    serde_json::Value::Array(
        tokens
            .tokens
            .iter()
            .map(|token| {
                serde_json::json!({
                    "name": token.name,
                    "scope": token.access,
                    "workspaces": token.workspaces,
                })
            })
            .collect(),
    )
}

/// Opens the credential store the selected config chooses, as every rho
/// process does before it reads a secret.
pub(super) fn open_credential_store(cli: &Cli) -> anyhow::Result<()> {
    let repository = ConfigRepository::new(cli.config.clone());
    let path = repository.configured_path()?;
    let mut config = repository.load()?;
    crate::credential_store::initialize_from_config(&mut config, &path)
}
