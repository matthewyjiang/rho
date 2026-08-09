//! CLI handlers for `rho mcp list` and `rho mcp show`.

use serde::Serialize;

use crate::{
    cli::{Cli, McpCommand},
    tools::mcp::{
        McpCatalog, McpConnectOutcome, McpLoadMode, McpRoots, McpServerReport, McpServerStatus,
        McpSessionOptions, McpSessionPlan, McpSessionReport, McpTransportSummary,
    },
};

use super::config_repository::ConfigRepository;

pub(super) async fn run(command: &McpCommand, cli: &Cli) -> anyhow::Result<()> {
    let config_repository = ConfigRepository::new(cli.config.clone());
    let config = config_repository.load()?;
    // Include plugin-provided servers so the inventory matches a session.
    let cwd = std::env::current_dir()?;
    let plugin_discovery = crate::plugins::discover_mcp(&cwd, crate::paths::home_dir().as_deref());
    crate::plugins::log(&plugin_discovery.report);
    let mut mcp_config = config.mcp.clone();
    mcp_config.merge(plugin_discovery.mcp);
    let connect = match command {
        McpCommand::List { connect, .. } | McpCommand::Show { connect, .. } => *connect,
    };
    let plan = if connect {
        McpSessionPlan::Connect
    } else {
        McpSessionPlan::Inventory(McpLoadMode::Native)
    };
    let outcome = McpConnectOutcome::run(
        plan,
        &mcp_config,
        McpSessionOptions::new(config.max_output_bytes, McpRoots::for_workspace(&cwd)),
    )
    .await;
    let result = match command {
        McpCommand::List { json, .. } => print_list(&outcome.report, *json),
        McpCommand::Show { id, json, .. } => {
            print_show(&outcome.report, &outcome.catalog, id, *json)
        }
    };
    if let Some(bundle) = outcome.bundle {
        bundle.close().await;
    }
    result
}

#[derive(Serialize)]
struct McpListDocument<'a> {
    mode: McpLoadMode,
    servers: &'a [McpServerReport],
}

#[derive(Serialize)]
struct McpShowDocument<'a> {
    mode: McpLoadMode,
    server: &'a McpServerReport,
    /// Empty without `--connect`, because prompts and resources are session
    /// state rather than configuration.
    prompts: Vec<McpPromptEntry<'a>>,
    resources: Vec<McpResourceEntry<'a>>,
}

#[derive(Serialize)]
struct McpPromptEntry<'a> {
    name: &'a str,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

#[derive(Serialize)]
struct McpResourceEntry<'a> {
    uri: &'a str,
    name: &'a str,
    templated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime_type: Option<&'a str>,
}

fn print_list(report: &McpSessionReport, json: bool) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&McpListDocument {
                mode: report.mode,
                servers: &report.servers,
            })?
        );
        return Ok(());
    }

    if report.servers.is_empty() {
        println!("no MCP servers configured");
        println!("add servers under [mcp.servers] in the selected Rho config");
        return Ok(());
    }

    let id_width = report
        .servers
        .iter()
        .map(|server| server.identity.len())
        .max()
        .unwrap_or(8)
        .max(8);
    let status_width = report
        .servers
        .iter()
        .map(|server| server.status().as_str().len())
        .max()
        .unwrap_or(8)
        .max(8);

    for server in &report.servers {
        let transport = server
            .transport
            .as_ref()
            .map(McpTransportSummary::kind_label)
            .unwrap_or("-");
        let tools = match server.status() {
            McpServerStatus::Connected => server.tool_count().to_string(),
            _ => "-".into(),
        };
        println!(
            "{:<id_width$}  {:<status_width$}  {transport:<16}  tools {tools}",
            server.identity,
            server.status().as_str(),
        );
        if let Some(error) = server.error() {
            println!("{:id_width$}  error: {error}", "");
        }
    }
    Ok(())
}

fn print_show(
    report: &McpSessionReport,
    catalog: &McpCatalog,
    id: &str,
    json: bool,
) -> anyhow::Result<()> {
    let Some(server) = report.find(id) else {
        let known = report
            .servers
            .iter()
            .map(|server| server.identity.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if known.is_empty() {
            anyhow::bail!("no MCP server named '{id}'");
        }
        anyhow::bail!("no MCP server named '{id}'; known: {known}");
    };

    let prompts = catalog
        .prompts()
        .into_iter()
        .filter(|prompt| prompt.server == id)
        .collect::<Vec<_>>();
    let resources = catalog
        .resources()
        .into_iter()
        .filter(|resource| resource.server == id)
        .collect::<Vec<_>>();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&McpShowDocument {
                mode: report.mode,
                server,
                prompts: prompts
                    .iter()
                    .map(|prompt| McpPromptEntry {
                        name: &prompt.name,
                        command: format!("/{}", prompt.command_name()),
                        description: prompt.description.as_deref(),
                    })
                    .collect(),
                resources: resources
                    .iter()
                    .map(|resource| McpResourceEntry {
                        uri: &resource.uri,
                        name: &resource.name,
                        templated: resource.templated,
                        mime_type: resource.mime_type.as_deref(),
                    })
                    .collect(),
            })?
        );
        return Ok(());
    }

    println!("id: {}", server.identity);
    println!("status: {}", server.status().as_str());
    println!("enabled: {}", server.enabled());
    match server.transport.as_ref() {
        Some(transport) => {
            println!("transport: {}", transport.kind_label());
            println!("endpoint: {}", transport.endpoint_summary());
        }
        None => println!("transport: -"),
    }
    if let Some(error) = server.error() {
        println!("error: {error}");
    }
    println!("tools: {}", server.tool_count());
    for tool in server.tools() {
        println!("  {} ({})", tool.exported_name, tool.remote_name);
    }
    if server.filtered_out_count() > 0 {
        println!("filtered_out: {}", server.filtered_out_count());
    }
    if server.collision_skipped_count() > 0 {
        println!("collision_skipped: {}", server.collision_skipped_count());
    }
    if !prompts.is_empty() {
        println!("prompts: {}", prompts.len());
        for prompt in &prompts {
            println!("  /{}", prompt.command_name());
        }
    }
    if !resources.is_empty() {
        println!("resources: {}", resources.len());
        for resource in &resources {
            let kind = if resource.templated {
                " (template)"
            } else {
                ""
            };
            println!("  {} ({}){kind}", resource.uri, resource.name);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "mcp_cli_tests.rs"]
mod tests;
