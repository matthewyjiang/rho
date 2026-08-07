//! CLI handlers for `rho mcp list` and `rho mcp show`.

use serde::Serialize;

use crate::{
    cli::{Cli, McpCommand},
    tools::mcp::{
        McpBundle, McpLoadMode, McpServerReport, McpServerStatus, McpSessionReport,
        McpTransportSummary,
    },
};

use super::config_repository::ConfigRepository;

pub(super) async fn run(command: &McpCommand, cli: &Cli) -> anyhow::Result<()> {
    let config_repository = ConfigRepository::new(cli.config.clone());
    let config = config_repository.load()?;
    let outcome = McpBundle::connect(&config.mcp).await;
    let result = match command {
        McpCommand::List { json } => print_list(&outcome.report, *json),
        McpCommand::Show { id, json } => print_show(&outcome.report, id, *json),
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
        .map(|server| server.status.as_str().len())
        .max()
        .unwrap_or(8)
        .max(8);

    for server in &report.servers {
        let transport = server
            .transport
            .as_ref()
            .map(McpTransportSummary::kind_label)
            .unwrap_or("-");
        let tools = match server.status {
            McpServerStatus::Connected => server.tool_count().to_string(),
            _ => "-".into(),
        };
        println!(
            "{:<id_width$}  {:<status_width$}  {transport:<16}  tools {tools}",
            server.identity,
            server.status.as_str(),
        );
        if let Some(error) = server.error.as_deref() {
            println!("{:id_width$}  error: {error}", "");
        }
    }
    Ok(())
}

fn print_show(report: &McpSessionReport, id: &str, json: bool) -> anyhow::Result<()> {
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

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&McpShowDocument {
                mode: report.mode,
                server,
            })?
        );
        return Ok(());
    }

    println!("id: {}", server.identity);
    println!("status: {}", server.status.as_str());
    println!("enabled: {}", server.enabled);
    match server.transport.as_ref() {
        Some(transport) => {
            println!("transport: {}", transport.kind_label());
            println!("endpoint: {}", transport.endpoint_summary());
        }
        None => println!("transport: -"),
    }
    if let Some(error) = server.error.as_deref() {
        println!("error: {error}");
    }
    println!("tools: {}", server.tool_count());
    for tool in &server.tools {
        println!("  {} ({})", tool.exported_name, tool.remote_name);
    }
    if server.filtered_out_count > 0 {
        println!("filtered_out: {}", server.filtered_out_count);
    }
    if server.collision_skipped_count > 0 {
        println!("collision_skipped: {}", server.collision_skipped_count);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::report::McpToolReport;

    // Covers: show must fail closed when the identity is missing.
    // Owner: pure unit
    #[test]
    fn show_missing_identity_lists_known_servers() {
        let report = McpSessionReport {
            mode: McpLoadMode::Native,
            servers: vec![McpServerReport {
                identity: "filesystem".into(),
                enabled: true,
                transport: None,
                status: McpServerStatus::Connected,
                error: None,
                tools: vec![McpToolReport {
                    remote_name: "read".into(),
                    exported_name: "mcp__filesystem__read".into(),
                }],
                filtered_out_count: 0,
                collision_skipped_count: 0,
            }],
        };
        let error = print_show(&report, "missing", false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("missing"));
        assert!(error.contains("filesystem"));
    }
}
