//! Plugin activation state store tests.

use super::{
    parse_state_file, GitRecord, InstallRecord, PluginScope, PluginStateEntry, PluginStateFile,
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

    let store = PluginStateStore::load(project.path(), None, Some(home.path())).unwrap();

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

    let mut store = PluginStateStore::load(project.path(), None, Some(home.path())).unwrap();
    store.set_enabled(PluginScope::User, "demo", false).unwrap();

    let reloaded = PluginStateStore::load(project.path(), None, Some(home.path())).unwrap();
    assert!(!reloaded.is_enabled(PluginScope::User, "demo"));
    assert!(reloaded.is_enabled(PluginScope::Project, "demo"));
    assert!(home.path().join("plugins.toml").is_file());
    assert!(!project.path().join(".rho/plugins.toml").exists());
}

// Covers: link targets and git sources round-trip through plugins.toml.
// Owner: plugin state store.
#[test]
fn records_install_metadata() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let git = GitRecord {
        url: "https://github.com/owner/repo".into(),
        git_ref: Some("main".into()),
        subdir: Some("plugins/demo".into()),
        commit: "0123456789abcdef0123456789abcdef01234567".into(),
    };
    let cases = [
        (
            "linked",
            InstallRecord::Link {
                target: "/tmp/source".into(),
            },
            PluginStateEntry {
                enabled: true,
                install: Some(InstallRecord::Link {
                    target: "/tmp/source".into(),
                }),
            },
        ),
        (
            "fetched",
            InstallRecord::Git(git.clone()),
            PluginStateEntry {
                enabled: true,
                install: Some(InstallRecord::Git(git)),
            },
        ),
    ];

    for (name, record, expected) in cases {
        let mut store = PluginStateStore::load(project.path(), None, Some(home.path())).unwrap();
        store
            .record_install(PluginScope::Project, name, record)
            .unwrap();

        let reloaded = PluginStateStore::load(project.path(), None, Some(home.path())).unwrap();
        assert_eq!(
            reloaded.entry(PluginScope::Project, name),
            Some(&expected),
            "{name}"
        );
    }
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
        (
            "git origin without source",
            "version = 1\n\n[plugins.demo]\norigin = \"git\"\n",
        ),
        (
            "git table without git origin",
            "version = 1\n\n[plugins.demo]\norigin = \"install\"\n\n[plugins.demo.git]\nurl = \"https://github.com/o/r\"\ncommit = \"abc\"\n",
        ),
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
                    install: Some(InstallRecord::Copy),
                }
            )]
            .into_iter()
            .collect(),
        }
    );
}
