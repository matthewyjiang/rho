//! `rho mcp serve` and `rho mcp token`: the boundary that reads the process
//! environment (the caller's session depth, the environment sessions run
//! with) and the credential store, then hands resolved values to
//! `app::mcp_server` and `app::session_control`.

#[cfg(not(unix))]
use crate::cli::{Cli, McpServeArgs, McpTokenCommand};

#[cfg(not(unix))]
pub(super) async fn serve(_args: &McpServeArgs, _cli: &Cli) -> anyhow::Result<()> {
    anyhow::bail!(super::super::daemon_cli::UNSUPPORTED)
}

#[cfg(not(unix))]
pub(super) fn token(_command: &McpTokenCommand, _cli: &Cli) -> anyhow::Result<()> {
    anyhow::bail!(super::super::daemon_cli::UNSUPPORTED)
}

#[cfg(unix)]
pub(super) use unix::{serve, token};

#[cfg(unix)]
mod unix {
    use std::sync::Arc;

    use anyhow::Context;
    use rho_protocol::ClientEnvironment;

    use crate::{
        app::{
            config_repository::ConfigRepository,
            mcp_server::{
                self,
                tokens::{Rotation, TokenSet},
                HttpOptions,
            },
            session_control::{Access, Caller, Limits, SessionControl, SESSION_DEPTH_ENV},
        },
        cli::{Cli, McpServeArgs, McpTokenCommand, McpTokenScope},
        credential_store::UntrackedCredentialStore,
    };

    pub(in crate::app) async fn serve(args: &McpServeArgs, cli: &Cli) -> anyhow::Result<()> {
        let defaults = Limits::default();
        let limits = Limits {
            max_depth: args.max_depth.unwrap_or(defaults.max_depth),
            max_sessions: args.max_sessions.unwrap_or(defaults.max_sessions),
        };
        let caller = Caller {
            depth: caller_depth()?,
            environment: ClientEnvironment::of_this_process(),
        };
        let control = Arc::new(SessionControl::new(caller, limits));
        match args.http {
            None => mcp_server::serve_stdio(control).await,
            Some(address) => {
                // The tokens live in the credential store.
                open_credential_store(cli)?;
                mcp_server::serve_http(
                    control,
                    HttpOptions {
                        address,
                        allowed_hosts: args.allow_host.clone(),
                        allowed_origins: args.allow_origin.clone(),
                    },
                )
                .await
            }
        }
    }

    pub(in crate::app) fn token(command: &McpTokenCommand, cli: &Cli) -> anyhow::Result<()> {
        open_credential_store(cli)?;
        let store = UntrackedCredentialStore;
        let mut tokens = TokenSet::load(&store)?;
        match command {
            McpTokenCommand::Rotate {
                name,
                scope,
                workspace,
            } => {
                let secret = tokens.rotate(
                    name,
                    Rotation {
                        access: scope.map(|scope| match scope {
                            McpTokenScope::Read => Access::Read,
                            McpTokenScope::Write => Access::Write,
                        }),
                        workspaces: workspace.clone(),
                    },
                )?;
                tokens.save(&store)?;
                // The secret alone on stdout, so a script can capture it.
                println!("{secret}");
                eprintln!(
                    "saved token {name:?}; this is the only time its secret is shown. Send it as `Authorization: Bearer <secret>`."
                );
            }
            McpTokenCommand::List { json } => {
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
                if !tokens.revoke(name) {
                    anyhow::bail!("no MCP server token named {name:?}");
                }
                tokens.save(&store)?;
                println!("revoked token {name:?}");
            }
        }
        Ok(())
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

    /// The nesting depth of the session that started this process, from
    /// [`SESSION_DEPTH_ENV`]; 0 when no session did.
    fn caller_depth() -> anyhow::Result<u32> {
        match std::env::var(SESSION_DEPTH_ENV) {
            Ok(depth) => depth
                .parse()
                .with_context(|| format!("{SESSION_DEPTH_ENV}={depth:?} is not a depth")),
            Err(std::env::VarError::NotPresent) => Ok(0),
            Err(error) => Err(error).context(SESSION_DEPTH_ENV),
        }
    }

    /// Opens the credential store the selected config chooses, as every rho
    /// process does before it reads a secret.
    fn open_credential_store(cli: &Cli) -> anyhow::Result<()> {
        let repository = ConfigRepository::new(cli.config.clone());
        let path = repository.configured_path()?;
        let mut config = repository.load()?;
        crate::credential_store::initialize_from_config(&mut config, &path)
    }
}
