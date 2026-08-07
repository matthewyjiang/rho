//! Plugin activation state store tests.

use super::{
    parse_state_file, PluginOrigin, PluginScope, PluginStateEntry, PluginStateFile,
    PluginStateStore,
};
use tempfile::TempDir;

// Covers: missing state files mean every plugin stays enabled.
// Owner: plugin state store.
#[test]
fn missing_state_files_default_to_enabled() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();

    let store = PluginStateStore::load(project.path(), Some(home.path())).unwrap();

    assert!(store.is_enabled(PluginScope::User, "demo"));
    assert!(store.is_enabled(PluginScope::Project, "demo"));
}

// Covers: enable/disable persists outside package directories and reloads.
// Owner: plugin state store.
#[test]
fn persists_enablement_outside_package_tree() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();

    let mut store = PluginStateStore::load(project.path(), Some(home.path())).unwrap();
    store.set_enabled(PluginScope::User, "demo", false).unwrap();

    let reloaded = PluginStateStore::load(project.path(), Some(home.path())).unwrap();
    assert!(!reloaded.is_enabled(PluginScope::User, "demo"));
    assert!(reloaded.is_enabled(PluginScope::Project, "demo"));
    assert!(home.path().join("plugins.toml").is_file());
    assert!(!project.path().join(".rho/plugins.toml").exists());
}

// Covers: install metadata and link targets round-trip through plugins.toml.
// Owner: plugin state store.
#[test]
fn records_install_metadata() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();

    let mut store = PluginStateStore::load(project.path(), Some(home.path())).unwrap();
    store
        .record_install(
            PluginScope::Project,
            "linked",
            PluginOrigin::Link,
            Some("/tmp/source".into()),
        )
        .unwrap();

    let reloaded = PluginStateStore::load(project.path(), Some(home.path())).unwrap();
    let entry = reloaded.entry(PluginScope::Project, "linked").unwrap();
    assert_eq!(
        entry,
        &PluginStateEntry {
            enabled: true,
            origin: Some(PluginOrigin::Link),
            link_target: Some("/tmp/source".into()),
        }
    );
}

// Covers: unsupported state versions and invalid keys fail closed.
// Owner: plugin state parser.
#[test]
fn rejects_invalid_state_files() {
    let cases = [
        ("bad version", "version = 2\n"),
        (
            "bad plugin name",
            "version = 1\n\n[plugins.Bad_Name]\nenabled = false\n",
        ),
        ("unknown field", "version = 1\nunexpected = true\n"),
    ];
    for (name, text) in cases {
        assert!(parse_state_file(text).is_err(), "{name}");
    }

    let ok = parse_state_file(
        r#"
version = 1

[plugins.demo]
enabled = false
origin = "install"
"#,
    )
    .unwrap();
    assert_eq!(
        ok,
        PluginStateFile {
            version: 1,
            plugins: [(
                "demo".into(),
                PluginStateEntry {
                    enabled: false,
                    origin: Some(PluginOrigin::Install),
                    link_target: None,
                }
            )]
            .into_iter()
            .collect(),
        }
    );
}
