//! Agent Plugins loading tests: manifest validation, containment, component
//! failure isolation, skill conflicts, MCP translation, and placeholders.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::mcp_adapter::expand_placeholders;
use super::{discover, manifest, PluginStatus};
use crate::tools::mcp::config::{McpConfig, McpTransport};

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

fn discover_env(env: &Env) -> super::PluginDiscovery {
    discover(env.project.path(), Some(env.home.path()))
}

fn manifest_json(name: &str, extra: &str) -> String {
    format!(r#"{{"$schema": "{SCHEMA}", "name": "{name}"{extra}}}"#)
}

fn write_plugin(plugins_root: &Path, dir_name: &str, manifest: &str) -> PathBuf {
    let dir = plugins_root.join(dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("plugin.json"), manifest).unwrap();
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

fn write_mcp(plugin_dir: &Path, json: &str) {
    std::fs::write(plugin_dir.join("mcp.json"), json).unwrap();
}

fn stdio_server(command: &str) -> String {
    format!(
        r#"{{"$schema": "{MCP_SCHEMA}", "mcpServers": {{"main": {{"type": "stdio", "command": "{command}"}}}}}}"#
    )
}

fn loaded_plugin<'a>(discovery: &'a super::PluginDiscovery, name: &str) -> &'a super::LoadedPlugin {
    discovery
        .plugins
        .iter()
        .find(|plugin| plugin.name == name)
        .unwrap_or_else(|| {
            panic!(
                "plugin `{name}` was not loaded: {:?}",
                discovery.report.plugins
            )
        })
}

fn report_entry<'a>(
    discovery: &'a super::PluginDiscovery,
    name: &str,
) -> &'a super::PluginReportEntry {
    discovery
        .report
        .plugins
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("no report entry for `{name}`"))
}

// --- Manifest validation ---

#[test]
fn loads_minimal_valid_manifest() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    write_plugin(
        &user_plugins(&env),
        "minimal",
        &manifest_json("minimal", ""),
    );

    let discovery = discover_env(&env);

    let entry = report_entry(&discovery, "minimal");
    assert_eq!(entry.status, PluginStatus::Loaded);
    assert!(entry
        .problems
        .iter()
        .any(|p| p.contains("no usable components")));
}

// Covers: an unsupported manifest schema rejects the plugin before components.
// Owner: plugin manifest validation.
#[test]
fn rejects_manifest_before_components_when_schema_is_invalid() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "bad-schema",
        r#"{"$schema": "https://agent-plugins.org/schemas/2.0.0/plugin.schema.json", "name": "bad-schema"}"#,
    );
    write_skill(&dir, "hidden-skill", "must not load");

    let discovery = discover_env(&env);

    assert!(discovery.plugins.is_empty());
    let entry = report_entry(&discovery, "bad-schema");
    assert_eq!(entry.status, PluginStatus::Rejected);
    assert!(entry.problems[0].contains("unsupported"));
}

#[test]
fn rejects_manifest_with_missing_required_fields() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    write_plugin(
        &user_plugins(&env),
        "no-name",
        &format!(r#"{{"$schema": "{SCHEMA}"}}"#),
    );

    let discovery = discover_env(&env);

    assert_eq!(
        report_entry(&discovery, "no-name").status,
        PluginStatus::Rejected
    );
}

// Covers: fatal manifest schema violations reject the whole plugin.
// Owner: plugin manifest validation.
#[test]
fn rejects_manifest_with_fatal_type_violations() {
    #[derive(Debug)]
    struct Case {
        name: &'static str,
        manifest: String,
    }
    let cases = [
        Case {
            name: "version wrong type",
            manifest: manifest_json("fatal-plugin", r#", "version": 1"#),
        },
        Case {
            name: "author unknown field",
            manifest: manifest_json("fatal-plugin", r#", "author": {"handle": "x"}"#),
        },
        Case {
            name: "keywords wrong element type",
            manifest: manifest_json("fatal-plugin", r#", "keywords": [1]"#),
        },
        Case {
            name: "uppercase name",
            manifest: manifest_json("Fatal-Plugin", ""),
        },
        Case {
            name: "consecutive periods in name",
            manifest: manifest_json("too..many", ""),
        },
        Case {
            name: "manifest not an object",
            manifest: "[1, 2]".to_string(),
        },
    ];

    for case in cases {
        let env = env();
        std::fs::create_dir_all(user_plugins(&env)).unwrap();
        write_plugin(&user_plugins(&env), "fatal-plugin", &case.manifest);

        let discovery = discover_env(&env);

        assert!(
            discovery.plugins.is_empty(),
            "{}: plugin must be rejected",
            case.name
        );
        assert_eq!(
            discovery.report.plugins[0].status,
            PluginStatus::Rejected,
            "{}",
            case.name
        );
    }
}

// Covers: unknown fields and non-object extensions are non-fatal (spec 5.2, 8.1).
// Owner: plugin manifest validation.
#[test]
fn reports_and_ignores_non_fatal_manifest_violations() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    write_plugin(
        &user_plugins(&env),
        "tolerant",
        &manifest_json("tolerant", r#", "surprise": true, "extensions": [1]"#),
    );

    let discovery = discover_env(&env);

    let entry = report_entry(&discovery, "tolerant");
    assert_eq!(entry.status, PluginStatus::Loaded);
    assert!(entry.problems.iter().any(|p| p.contains("`surprise`")));
    assert!(entry.problems.iter().any(|p| p.contains("`extensions`")));
}

#[test]
fn validates_plugin_names() {
    let valid = ["my-plugin", "acme.tools", "lint3r", "a"];
    let invalid = [
        "My-Plugin",
        "-start",
        "end-",
        "has--double",
        "too.many..dots",
        "",
        "has space",
        &"x".repeat(65),
    ];
    for name in valid {
        assert!(manifest::validate_plugin_name(name).is_ok(), "{name}");
    }
    for name in invalid {
        assert!(manifest::validate_plugin_name(name).is_err(), "{name}");
    }
}

// --- Discovery policy and precedence ---

// Covers: discovery uses explicit roots only and never recurses.
// Owner: plugin discovery policy.
#[test]
fn discovers_only_explicit_roots_without_recursion() {
    let env = env();
    let plugins = user_plugins(&env);
    std::fs::create_dir_all(plugins.join("nested/deeper")).unwrap();
    write_plugin(
        &plugins.join("nested/deeper"),
        "buried",
        &manifest_json("buried", ""),
    );
    write_plugin(&plugins, "top-level", &manifest_json("top-level", ""));
    // A directory without plugin.json is not a plugin.
    std::fs::create_dir_all(plugins.join("not-a-plugin")).unwrap();

    let discovery = discover_env(&env);

    let names: Vec<_> = discovery.plugins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["top-level"]);
}

// Covers: duplicate plugin names resolve to the nearer root with a reported shadow.
// Owner: plugin discovery policy.
#[test]
fn project_plugin_shadows_user_plugin_with_same_name() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    std::fs::create_dir_all(project_plugins(&env)).unwrap();
    let user_dir = write_plugin(&user_plugins(&env), "dup", &manifest_json("dup", ""));
    write_skill(&user_dir, "user-flavor", "user version");
    let project_dir = write_plugin(&project_plugins(&env), "dup", &manifest_json("dup", ""));
    write_skill(&project_dir, "project-flavor", "project version");

    let discovery = discover_env(&env);

    assert_eq!(discovery.plugins.len(), 1);
    let plugin = loaded_plugin(&discovery, "dup");
    assert_eq!(plugin.skills.len(), 1);
    assert_eq!(plugin.skills[0].name, "project-flavor");
    assert_eq!(report_entry(&discovery, "dup").status, PluginStatus::Loaded);
    let shadowed = discovery
        .report
        .plugins
        .iter()
        .find(|entry| entry.status == PluginStatus::Shadowed)
        .expect("shadowed entry reported");
    assert_eq!(shadowed.name, "dup");
}

// --- Skill discovery and failure isolation ---

// Covers: skills are immediate children only, with no recursion.
// Owner: plugin skill discovery.
#[test]
fn discovers_immediate_child_skills_only() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "skillful",
        &manifest_json("skillful", ""),
    );
    write_skill(&dir, "top-skill", "top level");
    // Nested skill directories are never discovered.
    let nested = dir.join("skills/top-skill/nested-skill");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        nested.join("SKILL.md"),
        "---\nname: nested-skill\ndescription: hidden\n---\n",
    )
    .unwrap();
    // A child without SKILL.md is not a skill.
    std::fs::create_dir_all(dir.join("skills/empty-child")).unwrap();

    let discovery = discover_env(&env);

    let plugin = loaded_plugin(&discovery, "skillful");
    let names: Vec<_> = plugin.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["top-skill"]);
}

// Covers: an invalid skill must not block valid siblings or MCP servers.
// Owner: plugin failure isolation.
#[test]
fn invalid_skill_does_not_block_valid_siblings_or_mcp() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(&user_plugins(&env), "mixed", &manifest_json("mixed", ""));
    write_skill(&dir, "good-skill", "loads fine");
    write_skill(&dir, "bad--skill", "invalid name");
    write_mcp(&dir, &stdio_server("validator"));

    let discovery = discover_env(&env);

    let plugin = loaded_plugin(&discovery, "mixed");
    let names: Vec<_> = plugin.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["good-skill"]);
    assert_eq!(plugin.mcp_servers.len(), 1);
    let entry = report_entry(&discovery, "mixed");
    assert!(entry.problems.iter().any(|p| p.contains("bad--skill")));
}

// Covers: a wrong-kind skills location invalidates only that component type.
// Owner: plugin failure isolation.
#[test]
fn wrong_kind_skills_location_invalidates_only_skills_component() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "wrongkind",
        &manifest_json("wrongkind", ""),
    );
    std::fs::write(dir.join("skills"), "a file, not a directory").unwrap();
    write_mcp(&dir, &stdio_server("validator"));

    let discovery = discover_env(&env);

    let plugin = loaded_plugin(&discovery, "wrongkind");
    assert!(plugin.skills.is_empty());
    assert_eq!(plugin.mcp_servers.len(), 1);
    let entry = report_entry(&discovery, "wrongkind");
    assert!(entry.problems.iter().any(|p| p.contains("not a directory")));
}

// Covers: a SKILL.md symlink escaping the plugin root is skipped (spec 4.1).
// Owner: plugin path containment.
#[cfg(unix)]
#[test]
fn skill_md_symlink_escaping_root_is_skipped() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "escaper",
        &manifest_json("escaper", ""),
    );
    let outside = env.home.path().join("outside-skill.md");
    std::fs::write(
        &outside,
        "---\nname: escape-skill\ndescription: outside\n---\n",
    )
    .unwrap();
    let skill_dir = dir.join("skills/escape-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::os::unix::fs::symlink(&outside, skill_dir.join("SKILL.md")).unwrap();
    write_skill(&dir, "inside-skill", "stays");

    let discovery = discover_env(&env);

    let plugin = loaded_plugin(&discovery, "escaper");
    let names: Vec<_> = plugin.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["inside-skill"]);
    let entry = report_entry(&discovery, "escaper");
    assert!(entry.problems.iter().any(|p| p.contains("escape-skill")));
}

// Covers: loose skills keep precedence over plugin skills.
// Owner: skill precedence.
#[test]
fn plugin_skills_lose_to_loose_skills_and_report_conflict() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "conflicted",
        &manifest_json("conflicted", ""),
    );
    write_skill(&dir, "shared-name", "plugin version");
    let loose_dir = env.home.path().join(".agents/skills/shared-name");
    std::fs::create_dir_all(&loose_dir).unwrap();
    std::fs::write(
        loose_dir.join("SKILL.md"),
        "---\nname: shared-name\ndescription: loose version\n---\n",
    )
    .unwrap();

    let skills = crate::skills::discover_with_home(env.project.path(), Some(env.home.path()));

    let skill = skills
        .iter()
        .find(|skill| skill.name == "shared-name")
        .unwrap();
    assert_eq!(skill.description, "loose version");
    assert!(matches!(skill.source, crate::skills::SkillSource::File(_)));
}

#[test]
// Covers: the skill source records owning plugin, plugin root, and skill root.
// Owner: skill source model.
fn plugin_skill_source_records_ownership() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(&user_plugins(&env), "owned", &manifest_json("owned", ""));
    write_skill(&dir, "owned-skill", "desc");

    let skills = crate::skills::discover_with_home(env.project.path(), Some(env.home.path()));

    let skill = skills
        .iter()
        .find(|skill| skill.name == "owned-skill")
        .unwrap();
    match &skill.source {
        crate::skills::SkillSource::Plugin {
            plugin,
            plugin_root,
            skill_root,
        } => {
            assert_eq!(plugin, "owned");
            assert!(plugin_root.ends_with("owned"));
            assert!(skill_root.ends_with("skills/owned-skill"));
        }
        other => panic!("expected plugin source, got {other:?}"),
    }
}

// --- MCP translation and failure isolation ---

// Covers: stdio translation expands placeholders and provides PLUGIN_ROOT/PLUGIN_DATA.
// Owner: MCP package adapter.
#[test]
fn translates_stdio_server_with_placeholders() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "devtools",
        &manifest_json("devtools", ""),
    );
    write_mcp(
        &dir,
        &format!(
            r#"{{
                "$schema": "{MCP_SCHEMA}",
                "mcpServers": {{
                    "validator": {{
                        "type": "stdio",
                        "command": "./bin/validator",
                        "args": ["--data", "${{PLUGIN_DATA}}/validator"],
                        "env": {{"CONFIG": "${{PLUGIN_ROOT}}/config.json"}},
                        "cwd": "${{PLUGIN_ROOT}}"
                    }}
                }}
            }}"#
        ),
    );

    let discovery = discover_env(&env);

    let plugin = loaded_plugin(&discovery, "devtools");
    let (name, config) = &plugin.mcp_servers[0];
    assert_eq!(name, "validator");
    match &config.transport {
        McpTransport::Stdio {
            command,
            args,
            cwd,
            env,
            env_from_env,
        } => {
            // The client-provided reserved variables are the runtime's view
            // of the plugin root and data directory.
            let root = env
                .get("PLUGIN_ROOT")
                .expect("PLUGIN_ROOT provided")
                .clone();
            let data = env
                .get("PLUGIN_DATA")
                .expect("PLUGIN_DATA provided")
                .clone();
            assert!(Path::new(&root).ends_with("devtools"));
            assert!(Path::new(&data).ends_with(Path::new("data/devtools")));
            assert_eq!(command, &format!("{root}/bin/validator"));
            assert_eq!(args, &["--data".to_string(), format!("{data}/validator")]);
            assert_eq!(cwd, &Some(PathBuf::from(&root)));
            assert_eq!(env.get("CONFIG").unwrap(), &format!("{root}/config.json"));
            assert!(env_from_env.is_empty());
            // PLUGIN_DATA exists before any subprocess would start.
            assert!(Path::new(&data).is_dir());
        }
        other => panic!("expected stdio transport, got {other:?}"),
    }
}

// Covers: an omitted cwd defaults to the plugin root.
// Owner: MCP package adapter.
#[test]
fn stdio_server_defaults_cwd_to_plugin_root() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "defaults",
        &manifest_json("defaults", ""),
    );
    write_mcp(&dir, &stdio_server("npx"));

    let discovery = discover_env(&env);

    let plugin = loaded_plugin(&discovery, "defaults");
    let (_, config) = &plugin.mcp_servers[0];
    match &config.transport {
        McpTransport::Stdio {
            command, cwd, env, ..
        } => {
            assert_eq!(command, "npx");
            let root = env.get("PLUGIN_ROOT").expect("PLUGIN_ROOT provided");
            assert_eq!(cwd.as_deref(), Some(Path::new(root)));
        }
        other => panic!("expected stdio transport, got {other:?}"),
    }
}

// Covers: invalid stdio entries isolate per server.
// Owner: MCP package adapter.
#[test]
fn invalid_stdio_entries_isolate_per_server() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(&user_plugins(&env), "broken", &manifest_json("broken", ""));
    write_mcp(
        &dir,
        &format!(
            r#"{{
                "$schema": "{MCP_SCHEMA}",
                "mcpServers": {{
                    "escapes": {{"type": "stdio", "command": "../outside"}},
                    "reserved-env": {{"type": "stdio", "command": "ok", "env": {{"PLUGIN_ROOT": "x"}}}},
                    "bad-cwd": {{"type": "stdio", "command": "ok", "cwd": "data"}},
                    "unknown-field": {{"type": "stdio", "command": "ok", "url": "https://x.example"}},
                    "fine": {{"type": "stdio", "command": "ok"}}
                }}
            }}"#
        ),
    );

    let discovery = discover_env(&env);

    let plugin = loaded_plugin(&discovery, "broken");
    let names: Vec<_> = plugin
        .mcp_servers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["fine"]);
    let identities: Vec<_> = plugin
        .invalid_mcp_servers
        .iter()
        .map(|invalid| invalid.identity.as_str())
        .collect();
    assert_eq!(
        identities,
        [
            "broken/bad-cwd",
            "broken/escapes",
            "broken/reserved-env",
            "broken/unknown-field"
        ]
    );
}

// Covers: an unsupported sse transport skips one entry without blocking siblings.
// Owner: MCP package adapter.
#[test]
fn unsupported_sse_transport_is_skipped_without_blocking_siblings() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(&user_plugins(&env), "legacy", &manifest_json("legacy", ""));
    write_mcp(
        &dir,
        &format!(
            r#"{{
                "$schema": "{MCP_SCHEMA}",
                "mcpServers": {{
                    "old": {{"type": "sse", "url": "https://legacy.example.com/sse"}},
                    "current": {{"type": "streamable-http", "url": "https://current.example.com/mcp"}}
                }}
            }}"#
        ),
    );

    let discovery = discover_env(&env);

    let plugin = loaded_plugin(&discovery, "legacy");
    let names: Vec<_> = plugin
        .mcp_servers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["current"]);
    let entry = report_entry(&discovery, "legacy");
    assert!(entry.problems.iter().any(|p| p.contains("legacy/old")));
}

// Covers: an invalid top-level mcp.json disables only the MCP component.
// Owner: MCP package adapter.
#[test]
fn invalid_top_level_mcp_disables_only_mcp_component() {
    #[derive(Debug)]
    struct Case {
        name: &'static str,
        json: String,
    }
    let cases = [
        Case {
            name: "invalid JSON",
            json: "{not json".to_string(),
        },
        Case {
            name: "unknown top-level field",
            json: format!(r#"{{"$schema": "{MCP_SCHEMA}", "mcpServers": {{}}, "extra": 1}}"#),
        },
        Case {
            name: "missing mcpServers",
            json: format!(r#"{{"$schema": "{MCP_SCHEMA}"}}"#),
        },
        Case {
            name: "version mismatch",
            json: r#"{"$schema": "https://agent-plugins.org/schemas/1.1.0/mcp.schema.json", "mcpServers": {}}"#
                .to_string(),
        },
    ];

    for case in cases {
        let env = env();
        std::fs::create_dir_all(user_plugins(&env)).unwrap();
        let dir = write_plugin(
            &user_plugins(&env),
            "mcp-bad",
            &manifest_json("mcp-bad", ""),
        );
        write_skill(&dir, "still-loads", "skills survive");
        write_mcp(&dir, &case.json);

        let discovery = discover_env(&env);

        let plugin = loaded_plugin(&discovery, "mcp-bad");
        assert!(
            plugin.mcp_servers.is_empty(),
            "{}: MCP must be disabled",
            case.name
        );
        assert_eq!(
            plugin.skills.len(),
            1,
            "{}: skills must keep loading",
            case.name
        );
        let entry = report_entry(&discovery, "mcp-bad");
        assert!(
            entry.problems.iter().any(|p| p.contains("MCP disabled")),
            "{}: disabled reason reported",
            case.name
        );
    }
}

// Covers: streamable-http entries carry literal headers.
// Owner: MCP package adapter.
#[test]
fn translates_streamable_http_server_with_literal_headers() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(&user_plugins(&env), "remote", &manifest_json("remote", ""));
    write_mcp(
        &dir,
        &format!(
            r#"{{
                "$schema": "{MCP_SCHEMA}",
                "mcpServers": {{
                    "api": {{
                        "type": "streamable-http",
                        "url": "https://deploy.example.com/mcp",
                        "headers": {{"X-Tenant": "public-tenant"}}
                    }}
                }}
            }}"#
        ),
    );

    let discovery = discover_env(&env);

    let plugin = loaded_plugin(&discovery, "remote");
    let (_, config) = &plugin.mcp_servers[0];
    match &config.transport {
        McpTransport::StreamableHttp {
            url,
            headers,
            headers_from_env,
        } => {
            assert_eq!(url, "https://deploy.example.com/mcp");
            assert_eq!(headers.get("X-Tenant").unwrap(), "public-tenant");
            assert!(headers_from_env.is_empty());
        }
        other => panic!("expected streamable-http transport, got {other:?}"),
    }
}

// Covers: invalid remote entries isolate per server.
// Owner: MCP package adapter.
#[test]
fn invalid_remote_entries_isolate_per_server() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "remote-bad",
        &manifest_json("remote-bad", ""),
    );
    write_mcp(
        &dir,
        &format!(
            r#"{{
                "$schema": "{MCP_SCHEMA}",
                "mcpServers": {{
                    "userinfo": {{"type": "streamable-http", "url": "https://user:pass@example.com/mcp"}},
                    "fragment": {{"type": "streamable-http", "url": "https://example.com/mcp#frag"}},
                    "plain-http": {{"type": "streamable-http", "url": "http://example.com/mcp"}},
                    "dup-header": {{"type": "streamable-http", "url": "https://example.com/mcp", "headers": {{"X-A": "1", "x-a": "2"}}}},
                    "fine": {{"type": "streamable-http", "url": "http://localhost:7777/mcp"}}
                }}
            }}"#
        ),
    );

    let discovery = discover_env(&env);

    let plugin = loaded_plugin(&discovery, "remote-bad");
    let names: Vec<_> = plugin
        .mcp_servers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["fine"]);
    assert_eq!(plugin.invalid_mcp_servers.len(), 4);
}

// Covers: a plugin without MCP servers adds no MCP work (zero-server fast path).
// Owner: MCP package adapter.
#[test]
fn plugin_without_mcp_contributes_no_mcp_work() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "skills-only",
        &manifest_json("skills-only", ""),
    );
    write_skill(&dir, "plain-skill", "no servers");

    let discovery = discover_env(&env);

    let mut config = McpConfig::default();
    discovery.merge_mcp_into(&mut config);
    assert!(config.is_empty());
    assert!(!config.has_enabled_servers());
}

// Covers: merged server identities stay plugin-scoped.
// Owner: MCP package adapter.
#[test]
fn merge_mcp_uses_plugin_scoped_identities() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "devtools",
        &manifest_json("devtools", ""),
    );
    write_mcp(&dir, &stdio_command_json("validator"));

    let discovery = discover_env(&env);

    let mut config = McpConfig::default();
    discovery.merge_mcp_into(&mut config);
    assert!(config.servers.contains_key("devtools/validator"));
    assert!(config.has_enabled_servers());
}

fn stdio_command_json(command: &str) -> String {
    format!(
        r#"{{"$schema": "{MCP_SCHEMA}", "mcpServers": {{"validator": {{"type": "stdio", "command": "{command}"}}}}}}"#
    )
}

// --- Placeholder expansion ---

// Covers: placeholder expansion is single-pass and non-recursive (spec 9.2).
// Owner: placeholder expansion.
#[test]
fn expands_placeholders_single_pass() {
    #[derive(Debug)]
    struct Case {
        name: &'static str,
        input: &'static str,
        expected: String,
    }
    let root = "/plugins/demo";
    let data = "/plugins/data/demo";
    let cases = [
        Case {
            name: "root in args",
            input: "--config ${PLUGIN_ROOT}/db.json",
            expected: format!("--config {root}/db.json"),
        },
        Case {
            name: "data alone",
            input: "${PLUGIN_DATA}",
            expected: data.to_string(),
        },
        Case {
            name: "both placeholders",
            input: "${PLUGIN_ROOT}:${PLUGIN_DATA}",
            expected: format!("{root}:{data}"),
        },
        Case {
            name: "unknown placeholder stays literal",
            input: "${HOME}/x",
            expected: "${HOME}/x".to_string(),
        },
        Case {
            name: "unbraced stays literal",
            input: "$PLUGIN_ROOT/x",
            expected: "$PLUGIN_ROOT/x".to_string(),
        },
        Case {
            name: "dangling brace stays literal",
            input: "end ${",
            expected: "end ${".to_string(),
        },
    ];
    for case in cases {
        assert_eq!(
            expand_placeholders(case.input, root, data),
            case.expected,
            "{}",
            case.name
        );
    }

    // Replacement text is never rescanned: a root containing the data
    // placeholder must pass through unexpanded.
    assert_eq!(
        expand_placeholders("${PLUGIN_ROOT}", "${PLUGIN_DATA}", data),
        "${PLUGIN_DATA}"
    );
}

// --- Diagnostics surface ---

// Covers: the doctor surface reports loaded plugins and supported components.
// Owner: plugin diagnostics.
#[test]
fn doctor_presentation_reports_supported_components() {
    let env = env();
    std::fs::create_dir_all(user_plugins(&env)).unwrap();
    let dir = write_plugin(
        &user_plugins(&env),
        "presented",
        &manifest_json("presented", ""),
    );
    write_skill(&dir, "presented-skill", "loads cleanly");

    let discovery = discover_env(&env);
    let presentation = discovery.report.doctor_presentation();

    assert_eq!(presentation.status, "1 loaded");
    assert!(presentation.healthy);
    assert!(presentation.detail.contains("stdio"));
    assert!(presentation.detail.contains("streamable-http"));

    // A plugin with no usable components loads with a reported problem.
    write_plugin(
        &user_plugins(&env),
        "empty-plugin",
        &manifest_json("empty-plugin", ""),
    );
    let with_problem = discover_env(&env).report.doctor_presentation();
    assert!(!with_problem.healthy);
    assert!(with_problem.status.contains("problem"));

    let empty = crate::plugins::PluginLoadReport::default();
    let empty_presentation = empty.doctor_presentation();
    assert!(empty_presentation.healthy);
    assert!(empty_presentation.status.contains("none"));
}
