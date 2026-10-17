//! Git install and update tests against local repositories over `file://`.

use std::{path::Path, process::Command};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{install_git, update, update_all, UpdateOutcome};
use crate::plugins::{
    discover_with_trust,
    git_source::GitSource,
    manage::{
        install, set_enabled,
        tests::{env, write_source, Env},
        InstallMode,
    },
    state::PluginScope,
    PluginOrigin, PluginStatus, ProjectTrust,
};

/// Run git in `dir` with an identity that ignores ambient signing config.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=rho",
            "-c",
            "user.email=rho@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Commit everything in `repo` and return the new commit id.
fn commit_all(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "--quiet", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

/// A repository on branch `main` containing plugin `demo` at `plugins/demo`.
fn plugin_repo() -> (TempDir, String) {
    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init", "--quiet", "--initial-branch=main"]);
    write_source(&repo.path().join("plugins"), "demo");
    let commit = commit_all(repo.path(), "init");
    (repo, commit)
}

fn file_url(path: &Path) -> String {
    url::Url::from_directory_path(path).unwrap().to_string()
}

fn demo_source(repo: &Path) -> GitSource {
    GitSource {
        url: file_url(repo),
        git_ref: Some("main".to_string()),
        subdir: Some("plugins/demo".to_string()),
    }
}

fn demo_status(env: &Env) -> PluginStatus {
    discover_with_trust(
        env.project.path(),
        Some(env.home.path()),
        Some(env.rho_home.path()),
        ProjectTrust::Trusted,
    )
    .report
    .find("demo")
    .unwrap()
    .status
}

// Covers: git install records its commit; update is a no-op until the ref
// moves, then replaces the package while keeping a user disable.
// Owner: plugin git install and update policy.
#[test]
fn git_install_then_update_follows_recorded_ref() {
    let env = env();
    let (repo, first) = plugin_repo();
    let installed = install_git(
        &demo_source(repo.path()),
        PluginScope::User,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();
    assert_eq!(installed.origin, PluginOrigin::Git);
    assert_eq!(installed.git.as_ref().unwrap().commit, first);
    let manifest = env.home.path().join(".agents/plugins/demo/plugin.json");
    assert!(manifest.is_file());

    // A local install is not an update candidate.
    let local_root = TempDir::new().unwrap();
    install(
        &write_source(local_root.path(), "local"),
        PluginScope::User,
        InstallMode::Copy,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();
    let rho_home = Some(env.rho_home.path());
    assert!(update("local", env.project.path(), env.home.path(), rho_home).is_err());

    set_enabled(
        "demo",
        /* enabled */ false,
        env.project.path(),
        Some(env.home.path()),
        rho_home,
    )
    .unwrap();

    let results = update_all(env.project.path(), env.home.path(), rho_home).unwrap();
    let outcomes = results
        .into_iter()
        .map(|(name, result)| (name, result.unwrap().outcome))
        .collect::<Vec<_>>();
    assert_eq!(outcomes, [("demo".to_string(), UpdateOutcome::UpToDate)]);

    let plugin_json = repo.path().join("plugins/demo/plugin.json");
    let bumped = std::fs::read_to_string(&plugin_json)
        .unwrap()
        .replace("1.2.3", "2.0.0");
    std::fs::write(&plugin_json, bumped).unwrap();
    let second = commit_all(repo.path(), "bump");

    let updated = update("demo", env.project.path(), env.home.path(), rho_home).unwrap();
    assert_eq!(updated.outcome, UpdateOutcome::Updated { previous: first });
    assert_eq!(updated.package.version.as_deref(), Some("2.0.0"));
    assert_eq!(updated.package.git.unwrap().commit, second);
    assert!(std::fs::read_to_string(&manifest)
        .unwrap()
        .contains("2.0.0"));
    assert_eq!(demo_status(&env), PluginStatus::Disabled);
}

// Covers: repository-root packages install without git metadata, and a
// missing package path fails before touching the managed root.
// Owner: plugin git install packaging.
#[test]
fn git_install_copies_package_files_only() {
    let env = env();
    let parent = TempDir::new().unwrap();
    let repo = write_source(parent.path(), "demo");
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    commit_all(&repo, "root plugin");

    let missing = GitSource {
        subdir: Some("plugins/nope".to_string()),
        ..demo_source(&repo)
    };
    let error = install_git(
        &missing,
        PluginScope::User,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap_err();
    assert!(error.to_string().contains("plugins/nope"), "{error}");
    assert!(!env.home.path().join(".agents/plugins").exists());

    let root = GitSource {
        subdir: None,
        ..demo_source(&repo)
    };
    install_git(
        &root,
        PluginScope::User,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();
    let installed = env.home.path().join(".agents/plugins/demo");
    assert!(installed.join("plugin.json").is_file());
    assert!(!installed.join(".git").exists());
    assert_eq!(demo_status(&env), PluginStatus::Loaded);
}
