//! CLI handlers for `rho plugins` lifecycle commands.
//!
//! These commands inspect and manage local packages only. They parse manifests
//! and component metadata without executing package code or connecting MCP.

use std::path::Path;

use serde::Serialize;

use crate::{
    cli::{Cli, PluginsCommand, PluginsScope},
    plugins::{
        discover_with_rho_home,
        manage::{self, InstallMode},
        PluginLoadReport, PluginScope, PluginStatus,
    },
};

pub(super) fn run(command: &PluginsCommand, _cli: &Cli) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let home = crate::paths::home_dir();
    let rho_home = crate::paths::rho_dir().ok();

    match command {
        PluginsCommand::List { json } => {
            let discovery = discover_with_rho_home(&cwd, home.as_deref(), rho_home.as_deref());
            crate::plugins::log(&discovery.report);
            print_list(&discovery.report, *json)
        }
        PluginsCommand::Inspect { name, json } => {
            let discovery = discover_with_rho_home(&cwd, home.as_deref(), rho_home.as_deref());
            crate::plugins::log(&discovery.report);
            print_inspect(&discovery.report, name, *json)
        }
        PluginsCommand::Install { path, scope, force }
        | PluginsCommand::Link { path, scope, force } => {
            let home = require_home(home.as_deref())?;
            let mode = match command {
                PluginsCommand::Install { .. } => InstallMode::Copy,
                PluginsCommand::Link { .. } => InstallMode::Link,
                _ => unreachable!("matched install/link only"),
            };
            let package = manage::install(
                path,
                resolve_scope(*scope),
                mode,
                /* force */ *force,
                &cwd,
                home,
                rho_home.as_deref(),
            )?;
            match mode {
                InstallMode::Copy => {
                    println!(
                        "installed {} ({}) to {}",
                        package.name,
                        package.scope.as_str(),
                        crate::paths::display(&package.path)
                    );
                }
                InstallMode::Link => {
                    println!(
                        "linked {} ({}) at {} -> {}",
                        package.name,
                        package.scope.as_str(),
                        crate::paths::display(&package.path),
                        package
                            .link_target
                            .as_ref()
                            .map(|path| crate::paths::display(path))
                            .unwrap_or_else(|| crate::paths::display(path))
                    );
                }
            }
            Ok(())
        }
        PluginsCommand::Enable { name } => {
            let package = manage::set_enabled(
                name,
                /* enabled */ true,
                &cwd,
                home.as_deref(),
                rho_home.as_deref(),
            )?;
            println!(
                "enabled {} ({}) at {}",
                package.name,
                package.scope.as_str(),
                crate::paths::display(&package.path)
            );
            Ok(())
        }
        PluginsCommand::Disable { name } => {
            let package = manage::set_enabled(
                name,
                /* enabled */ false,
                &cwd,
                home.as_deref(),
                rho_home.as_deref(),
            )?;
            println!(
                "disabled {} ({}) at {}",
                package.name,
                package.scope.as_str(),
                crate::paths::display(&package.path)
            );
            println!("package files kept; components inactive in new sessions");
            Ok(())
        }
        PluginsCommand::Remove { name, yes } => {
            if !*yes && !confirm_remove(name)? {
                println!("remove cancelled");
                return Ok(());
            }
            let package = manage::remove(name, &cwd, home.as_deref(), rho_home.as_deref())?;
            println!(
                "removed {} ({}) from {}",
                package.name,
                package.scope.as_str(),
                crate::paths::display(&package.path)
            );
            println!("plugin data directories were left in place");
            Ok(())
        }
    }
}

fn resolve_scope(scope: PluginsScope) -> PluginScope {
    match scope {
        PluginsScope::User => PluginScope::User,
        PluginsScope::Project => PluginScope::Project,
    }
}

fn require_home(home: Option<&Path>) -> anyhow::Result<&Path> {
    home.ok_or_else(|| anyhow::anyhow!("could not determine home directory"))
}

fn confirm_remove(name: &str) -> anyhow::Result<bool> {
    use std::io::{self, IsTerminal, Write};
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!(
            "refusing to remove `{name}` without --yes when not running in an interactive terminal"
        );
    }
    print!("remove plugin `{name}`? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

#[derive(Serialize)]
struct PluginsListDocument<'a> {
    plugins: &'a [crate::plugins::PluginReportEntry],
}

#[derive(Serialize)]
struct PluginInspectDocument<'a> {
    plugin: &'a crate::plugins::PluginReportEntry,
}

fn print_list(report: &PluginLoadReport, json: bool) -> anyhow::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&PluginsListDocument {
                plugins: &report.plugins,
            })?
        );
        return Ok(());
    }

    if report.plugins.is_empty() {
        println!("no Agent Plugins discovered");
        println!(
            "install or link a package with `rho plugins install` / `rho plugins link`, or place one under .agents/plugins"
        );
        return Ok(());
    }

    let name_width = report
        .plugins
        .iter()
        .map(|plugin| plugin.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let status_width = report
        .plugins
        .iter()
        .map(|plugin| status_label(plugin.status).len())
        .max()
        .unwrap_or(6)
        .max(6);

    for plugin in &report.plugins {
        let version = plugin.version.as_deref().unwrap_or("-");
        println!(
            "{:<name_width$}  {:<status_width$}  {:<7}  {:<7}  {version:<8}  skills {}  mcp {}  {}",
            plugin.name,
            status_label(plugin.status),
            plugin.scope.as_str(),
            plugin.origin.as_str(),
            plugin.skill_count,
            plugin.mcp_server_count,
            plugin.root,
        );
    }
    Ok(())
}

fn print_inspect(report: &PluginLoadReport, name: &str, json: bool) -> anyhow::Result<()> {
    let Some(plugin) = report
        .plugins
        .iter()
        .find(|entry| entry.name == name && entry.status != PluginStatus::Shadowed)
        .or_else(|| report.find(name))
    else {
        let known = report
            .plugins
            .iter()
            .map(|plugin| plugin.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if known.is_empty() {
            anyhow::bail!("no plugin named `{name}`");
        }
        anyhow::bail!("no plugin named `{name}`; known: {known}");
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&PluginInspectDocument { plugin })?
        );
        return Ok(());
    }

    println!("name: {}", plugin.name);
    println!("version: {}", plugin.version.as_deref().unwrap_or("-"));
    if let Some(description) = &plugin.description {
        println!("description: {description}");
    }
    println!("status: {}", status_label(plugin.status));
    println!("enabled: {}", plugin.enabled);
    println!("scope: {}", plugin.scope.as_str());
    println!("origin: {}", plugin.origin.as_str());
    println!("location: {}", plugin.root);
    println!(
        "supported_components: {}",
        crate::plugins::SUPPORTED_COMPONENTS
    );
    println!("skills ({}):", plugin.skill_count);
    if plugin.skill_names.is_empty() {
        println!("  (none)");
    } else {
        for skill in &plugin.skill_names {
            println!("  {skill}  (plugin {})", plugin.name);
        }
    }
    println!("mcp_servers ({}):", plugin.mcp_server_count);
    if plugin.mcp_server_names.is_empty() {
        println!("  (none)");
    } else {
        for server in &plugin.mcp_server_names {
            println!("  {}/{server}", plugin.name);
        }
    }
    if !plugin.problems.is_empty() {
        println!("diagnostics:");
        for problem in &plugin.problems {
            println!("  - {problem}");
        }
    }
    Ok(())
}

fn status_label(status: PluginStatus) -> &'static str {
    match status {
        PluginStatus::Loaded => "loaded",
        PluginStatus::Disabled => "disabled",
        PluginStatus::Rejected => "rejected",
        PluginStatus::Shadowed => "shadowed",
    }
}

#[cfg(test)]
#[path = "plugins_cli_tests.rs"]
mod tests;
