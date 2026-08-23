//! Local plugin install/link/remove/enable tests.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::{inspect_source, install, remove, set_enabled, InstallMode};
use crate::plugins::{
    discover_with_trust, state::PluginScope, PluginOrigin, PluginStatus, ProjectTrust,
};

const SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

struct Env {
    home: TempDir,
    project: TempDir,
    rho_home: TempDir,
}

fn env() -> Env {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    let rho_home = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    Env {
        home,
        project,
        rho_home,
    }
}

fn write_source(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join("skills").join("hello")).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        format!(
            r#"{{"$schema":"{SCHEMA}","name":"{name}","version":"1.2.3","description":"demo plugin"}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("skills/hello/SKILL.md"),
        "---\nname: hello\ndescription: greets\n---\nbody\n",
    )
    .unwrap();
    dir
}

// Covers: install validates the package then copies into the managed user root.
// Owner: plugin install policy.
#[test]
fn install_copies_into_user_root_after_validation() {
    let env = env();
    let source_root = TempDir::new().unwrap();
    let source = write_source(source_root.path(), "demo");

    let package = install(
        &source,
        PluginScope::User,
        InstallMode::Copy,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();

    assert_eq!(package.name, "demo");
    assert_eq!(package.origin, PluginOrigin::Install);
    assert_eq!(package.version.as_deref(), Some("1.2.3"));
    let installed = env.home.path().join(".agents/plugins/demo/plugin.json");
    assert!(installed.is_file());
    // Source remains untouched.
    assert!(source.join("plugin.json").is_file());

    let discovery = discover_with_trust(
        env.project.path(),
        Some(env.home.path()),
        Some(env.rho_home.path()),
        ProjectTrust::Trusted,
    );
    let entry = discovery.report.find("demo").unwrap();
    assert_eq!(entry.status, PluginStatus::Loaded);
    assert_eq!(entry.origin, PluginOrigin::Install);
    assert_eq!(entry.skill_names, ["hello"]);
}

// Covers: invalid packages never reach a managed root.
// Owner: plugin install validation.
#[test]
fn install_rejects_invalid_package_before_copy() {
    let env = env();
    let source_root = TempDir::new().unwrap();
    let bad = source_root.path().join("bad");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("plugin.json"), r#"{"name":"bad"}"#).unwrap();

    let error = install(
        &bad,
        PluginScope::User,
        InstallMode::Copy,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap_err();

    assert!(error.to_string().contains("invalid plugin package"));
    assert!(!env.home.path().join(".agents/plugins/bad").exists());
}

// Covers: conflicting destinations require an explicit --force replacement.
// Owner: plugin install conflict policy.
#[test]
fn install_conflict_requires_force() {
    let env = env();
    let source_root = TempDir::new().unwrap();
    let source = write_source(source_root.path(), "demo");
    install(
        &source,
        PluginScope::User,
        InstallMode::Copy,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();

    let error = install(
        &source,
        PluginScope::User,
        InstallMode::Copy,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("--force"));

    install(
        &source,
        PluginScope::User,
        InstallMode::Copy,
        /* force */ true,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();
}

// Covers: link creates a managed symlink and records link origin.
// Owner: plugin link policy.
#[cfg(unix)]
#[test]
fn link_creates_managed_symlink() {
    let env = env();
    let source_root = TempDir::new().unwrap();
    let source = write_source(source_root.path(), "linked");

    let package = install(
        &source,
        PluginScope::Project,
        InstallMode::Link,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();

    let dest = env.project.path().join(".agents/plugins/linked");
    assert_eq!(package.origin, PluginOrigin::Link);
    assert!(std::fs::symlink_metadata(&dest)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        std::fs::canonicalize(&dest).unwrap(),
        std::fs::canonicalize(&source).unwrap()
    );
}

// Covers: disable keeps package files but drops components from discovery.
// Owner: plugin activation policy.
#[test]
fn disable_keeps_package_and_drops_session_components() {
    let env = env();
    let source_root = TempDir::new().unwrap();
    let source = write_source(source_root.path(), "demo");
    install(
        &source,
        PluginScope::User,
        InstallMode::Copy,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();

    set_enabled(
        "demo",
        /* enabled */ false,
        env.project.path(),
        Some(env.home.path()),
        Some(env.rho_home.path()),
    )
    .unwrap();

    assert!(env
        .home
        .path()
        .join(".agents/plugins/demo/plugin.json")
        .is_file());

    let discovery = discover_with_trust(
        env.project.path(),
        Some(env.home.path()),
        Some(env.rho_home.path()),
        ProjectTrust::Trusted,
    );
    assert!(discovery.skills.is_empty());
    assert!(!discovery.mcp.has_enabled_servers());
    let entry = discovery.report.find("demo").unwrap();
    assert_eq!(entry.status, PluginStatus::Disabled);
    assert!(!entry.enabled);
    assert_eq!(entry.skill_count, 1);
}

// Covers: remove deletes only the package slot and preserves PLUGIN_DATA.
// Owner: plugin remove safety.
#[test]
fn remove_deletes_package_but_keeps_data_directory() {
    let env = env();
    let source_root = TempDir::new().unwrap();
    let source = write_source(source_root.path(), "demo");
    install(
        &source,
        PluginScope::User,
        InstallMode::Copy,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();

    let plugins_root = env.home.path().join(".agents/plugins");
    let data = plugins_root.join("data/demo");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("state.json"), "{}").unwrap();

    remove(
        "demo",
        env.project.path(),
        Some(env.home.path()),
        Some(env.rho_home.path()),
    )
    .unwrap();

    assert!(!plugins_root.join("demo").exists());
    assert!(data.join("state.json").is_file());
}

// Covers: inspect_source never requires component execution and surfaces metadata.
// Owner: plugin package validation.
#[test]
fn inspect_source_reads_manifest_only() {
    let root = TempDir::new().unwrap();
    let source = write_source(root.path(), "meta");
    let inspected = inspect_source(&source).unwrap();
    assert_eq!(inspected.manifest.name, "meta");
    assert_eq!(inspected.manifest.version.as_deref(), Some("1.2.3"));
    assert_eq!(
        inspected.manifest.description.as_deref(),
        Some("demo plugin")
    );
}
