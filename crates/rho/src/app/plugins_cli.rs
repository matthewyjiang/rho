//! CLI handlers for `rho plugins` lifecycle commands.
//!
//! These commands inspect and manage packages in managed plugin roots. Install
//! and update may fetch git repositories. No command executes package code or
//! connects MCP; they read manifests and component metadata only.

use std::path::Path;

use serde::Serialize;

use crate::{
    cli::{Cli, PluginsCommand, PluginsScope},
    plugins::{
        discover_with_trust,
        git_source::{short_commit, InstallSource},
        manage::{self, InstallMode, UpdateOutcome, UpdateReport, UpdateSelection, UpdatedPackage},
        PluginLoadReport, PluginScope, PluginStatus, ProjectTrust, TRUST_PROJECT_PLUGINS_ENV,
    },
};

pub(super) fn run(command: &PluginsCommand, _cli: &Cli) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let home = crate::paths::home_dir();
    let rho_home = crate::paths::rho_dir().ok();
    let trust = ProjectTrust::from_plugins_env();

    match command {
        PluginsCommand::List { json } => {
            let discovery = discover_with_trust(&cwd, home.as_deref(), rho_home.as_deref(), trust);
            crate::plugins::log(&discovery.report);
            print_list(&discovery.report, *json)
        }
        PluginsCommand::Inspect { name, json } => {
            let discovery = discover_with_trust(&cwd, home.as_deref(), rho_home.as_deref(), trust);
            crate::plugins::log(&discovery.report);
            print_inspect(&discovery.report, name, *json)
        }
        PluginsCommand::Install {
            source,
            scope,
            force,
        } => {
            let home = require_home(home.as_deref())?;
            let scope = resolve_scope(*scope);
            match InstallSource::parse(source, &cwd)? {
                InstallSource::Local(path) => {
                    let package = manage::install(
                        &path,
                        scope,
                        InstallMode::Copy,
                        *force,
                        &cwd,
                        home,
                        rho_home.as_deref(),
                    )?;
                    println!(
                        "installed {} ({}) to {}",
                        package.name,
                        package.scope.as_str(),
                        crate::paths::display(&package.path)
                    );
                }
                InstallSource::Git(git) => {
                    eprintln!("fetching {}", git.describe());
                    let package =
                        manage::install_git(&git, scope, *force, &cwd, home, rho_home.as_deref())?;
                    let commit = package.git.as_ref().map_or("-", |git| git.commit.as_str());
                    println!(
                        "installed {} ({}) to {} from {} at {}",
                        package.name,
                        package.scope.as_str(),
                        crate::paths::display(&package.path),
                        git.url,
                        short_commit(commit)
                    );
                }
            }
            Ok(())
        }
        PluginsCommand::Link { path, scope, force } => {
            let home = require_home(home.as_deref())?;
            let package = manage::install(
                path,
                resolve_scope(*scope),
                InstallMode::Link,
                *force,
                &cwd,
                home,
                rho_home.as_deref(),
            )?;
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
            Ok(())
        }
        PluginsCommand::Update { name } => {
            let home = require_home(home.as_deref())?;
            let selection = match name {
                Some(name) => UpdateSelection::Named(name),
                None => UpdateSelection::All,
            };
            let report = manage::update(selection, trust, &cwd, home, rho_home.as_deref())?;
            print_update_report(report)
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
            if package.scope == PluginScope::Project && !trust.is_trusted() {
                println!(
                    "project plugins stay inactive in untrusted workspaces; set {TRUST_PROJECT_PLUGINS_ENV}=1 to activate"
                );
            }
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

/// Print one line per package; exit nonzero when any update failed.
fn print_update_report(report: UpdateReport) -> anyhow::Result<()> {
    if !report.untrusted.is_empty() {
        println!(
            "skipped project plugins in an untrusted workspace: {}; set {TRUST_PROJECT_PLUGINS_ENV}=1 to update them",
            report.untrusted.join(", ")
        );
    }
    if report.results.is_empty() {
        if report.untrusted.is_empty() {
            println!("no git-installed plugins to update");
        }
        return Ok(());
    }
    let total = report.results.len();
    let mut failed = 0;
    for (target, result) in report.results {
        let scope = target.scope.as_str();
        match result {
            Ok(UpdatedPackage { package, outcome }) => match outcome {
                UpdateOutcome::Updated { previous, current } => println!(
                    "updated {} ({scope}) {} -> {}",
                    package.name,
                    short_commit(&previous),
                    short_commit(&current)
                ),
                UpdateOutcome::UpToDate { commit } => println!(
                    "{} ({scope}) is up to date at {}",
                    package.name,
                    short_commit(&commit)
                ),
            },
            Err(error) => {
                failed += 1;
                eprintln!("could not update {} ({scope}): {error:#}", target.name);
            }
        }
    }
    if failed > 0 {
        anyhow::bail!("{failed} of {total} plugin updates failed");
    }
    Ok(())
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
    let plugin = inspect_entry(report, name)?;

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
    if let Some(notice) = plugin.status.policy_notice() {
        println!("{notice}");
    }
    println!("enabled: {}", plugin.enabled);
    println!("scope: {}", plugin.scope.as_str());
    println!("origin: {}", plugin.origin.as_str());
    if let Some(git) = &plugin.git {
        println!("source: {}", git.source().describe());
        println!("commit: {}", git.commit);
    }
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

fn inspect_entry<'a>(
    report: &'a PluginLoadReport,
    name: &str,
) -> anyhow::Result<&'a crate::plugins::PluginReportEntry> {
    if let Some(plugin) = report.inspect(name) {
        return Ok(plugin);
    }
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
}

fn status_label(status: PluginStatus) -> &'static str {
    match status {
        PluginStatus::Loaded => "loaded",
        PluginStatus::Disabled => "disabled",
        PluginStatus::Untrusted => "untrusted",
        PluginStatus::Rejected => "rejected",
        PluginStatus::Shadowed => "shadowed",
    }
}

#[cfg(test)]
#[path = "plugins_cli_tests.rs"]
mod tests;
