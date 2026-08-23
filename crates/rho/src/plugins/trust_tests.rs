//! Project plugin trust: inventory-only untrusted packages, user packages
//! still activate, and inactive packages do not claim names.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::plugins::{discover_with_trust, PluginStatus, ProjectTrust, TRUST_PROJECT_PLUGINS_ENV};
use crate::tools::mcp::config::McpTransport;

const SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
const MCP_SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";

struct Env {
    home: TempDir,
    project: TempDir,
}

fn env() -> Env {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    Env { home, project }
}

fn user_plugins(env: &Env) -> PathBuf {
    env.home.path().join(".agents").join("plugins")
}

fn project_plugins(env: &Env) -> PathBuf {
    env.project.path().join(".agents").join("plugins")
}

fn discover(env: &Env, trust: ProjectTrust) -> crate::plugins::PluginDiscovery {
    discover_with_trust(env.project.path(), Some(env.home.path()), None, trust)
}

fn manifest_json(name: &str) -> String {
    format!(r#"{{"$schema": "{SCHEMA}", "name": "{name}"}}"#)
}

fn write_plugin(plugins_root: &Path, dir_name: &str) -> PathBuf {
    let dir = plugins_root.join(dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("plugin.json"), manifest_json(dir_name)).unwrap();
    dir
}

fn write_skill(plugin_dir: &Path, name: &str, description: &str) {
    let skill_dir = plugin_dir.join("skills").join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nbody\n"),
    )
    .unwrap();
}

fn write_stdio_mcp(plugin_dir: &Path, command: &str) {
    std::fs::write(
        plugin_dir.join("mcp.json"),
        format!(
            r#"{{"$schema": "{MCP_SCHEMA}", "mcpServers": {{"main": {{"type": "stdio", "command": "{command}"}}}}}}"#
        ),
    )
    .unwrap();
}

fn report_entry<'a>(
    discovery: &'a crate::plugins::PluginDiscovery,
    name: &str,
) -> &'a crate::plugins::PluginReportEntry {
    discovery
        .report
        .plugins
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("no report entry for `{name}`"))
}

fn contributions<'a>(
    discovery: &'a crate::plugins::PluginDiscovery,
    name: &str,
) -> (
    Vec<&'a crate::skills::Skill>,
    Vec<(&'a str, &'a crate::tools::mcp::config::McpServerConfig)>,
) {
    let prefix = format!("{name}/");
    (
        discovery
            .skills
            .iter()
            .filter(|skill| {
                matches!(
                    &skill.source,
                    crate::skills::SkillSource::Filesystem { owner: Some(owner), .. }
                        if owner == name
                )
            })
            .collect(),
        discovery
            .mcp
            .servers
            .iter()
            .filter_map(|(identity, config)| {
                identity
                    .strip_prefix(&prefix)
                    .map(|server| (server, config))
            })
            .collect(),
    )
}

// Covers: untrusted project plugins load inventory-only and activate no
// components (skills or MCP servers, including stdio commands).
// Owner: plugin trust policy.
#[test]
fn untrusted_project_plugin_activates_no_components() {
    let env = env();
    std::fs::create_dir_all(project_plugins(&env)).unwrap();
    let dir = write_plugin(&project_plugins(&env), "risky");
    write_skill(&dir, "leaky", "must not reach the session");
    write_stdio_mcp(&dir, "bash");

    let discovery = discover(&env, ProjectTrust::Untrusted);

    assert!(discovery.skills.is_empty());
    assert!(!discovery.mcp.has_enabled_servers());
    assert!(discovery.mcp.servers.is_empty());
    let entry = report_entry(&discovery, "risky");
    assert_eq!(entry.status, PluginStatus::Untrusted);
    assert!(entry.enabled);
    assert_eq!(entry.skill_count, 1);
    assert_eq!(entry.mcp_server_count, 1);
    assert!(entry
        .problems
        .iter()
        .any(|problem| problem.contains(TRUST_PROJECT_PLUGINS_ENV)));
    assert_eq!(discovery.report.summary().untrusted, 1);
    assert_eq!(discovery.report.summary().problems, 0);
}

// Covers: user plugins are the user's own files and activate regardless of
// workspace trust.
// Owner: plugin trust policy.
#[test]
fn untrusted_workspace_still_activates_user_plugins() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(&user_plugins(&env), "mine");
    write_skill(&dir, "hello", "user version");
    write_stdio_mcp(&dir, "my-server");

    let discovery = discover(&env, ProjectTrust::Untrusted);

    assert_eq!(
        report_entry(&discovery, "mine").status,
        PluginStatus::Loaded
    );
    let (skills, mcp_servers) = contributions(&discovery, "mine");
    assert_eq!(skills.len(), 1);
    assert_eq!(mcp_servers.len(), 1);
    assert!(matches!(
        mcp_servers[0].1.transport,
        McpTransport::Stdio { .. }
    ));
}

// Covers: an untrusted project plugin must not shadow a user plugin of the
// same name; only packages that claim a name occupy it.
// Owner: plugin trust policy.
#[test]
fn untrusted_project_plugin_does_not_shadow_user_plugin() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    std::fs::create_dir_all(project_plugins(&env)).unwrap();
    let user_dir = write_plugin(&user_plugins(&env), "dup");
    write_skill(&user_dir, "user-flavor", "user version");
    let project_dir = write_plugin(&project_plugins(&env), "dup");
    write_skill(&project_dir, "project-flavor", "project version");

    let discovery = discover(&env, ProjectTrust::Untrusted);

    let loaded: Vec<_> = discovery
        .report
        .plugins
        .iter()
        .filter(|entry| entry.status == PluginStatus::Loaded)
        .map(|entry| entry.scope)
        .collect();
    assert_eq!(loaded, [crate::plugins::PluginScope::User]);
    let untrusted = discovery
        .report
        .plugins
        .iter()
        .find(|entry| entry.status == PluginStatus::Untrusted)
        .expect("untrusted entry reported");
    assert_eq!(untrusted.name, "dup");
    let (skills, _) = contributions(&discovery, "dup");
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "user-flavor");
}
