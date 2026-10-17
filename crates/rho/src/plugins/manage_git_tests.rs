//! Git install and update tests against local repositories over `file://`.

use std::{path::Path, process::Command};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{install_git, update, UpdateOutcome, UpdateSelection, UpdateTarget};
use crate::plugins::{
    discover_with_trust,
    git_source::GitSource,
    manage::{
        install, set_enabled,
        tests::{env, write_source, Env},
        InstallMode,
    },
    state::{PluginScope, PluginStateStore},
    PluginOrigin, PluginStatus, ProjectTrust,
};

/// Run git in `dir` with an identity that ignores ambient signing config.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
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

/// A repository on branch `main` containing plugin `name` at `plugins/<name>`.
fn plugin_repo(name: &str) -> (TempDir, String) {
    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init", "--quiet", "--initial-branch=main"]);
    write_source(&repo.path().join("plugins"), name);
    let commit = commit_all(repo.path(), "init");
    (repo, commit)
}

/// `repo` on branch `main`, with the package at `subdir` (`None` = root).
fn repo_source(repo: &Path, subdir: Option<&str>) -> GitSource {
    GitSource::new(
        url::Url::from_directory_path(repo).unwrap().to_string(),
        Some("main".to_string()),
        subdir.map(str::to_string),
    )
    .unwrap()
}

fn install_from(env: &Env, source: &GitSource, scope: PluginScope) {
    install_git(
        source,
        scope,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();
}

fn run_update(
    env: &Env,
    selection: UpdateSelection<'_>,
    trust: ProjectTrust,
) -> anyhow::Result<super::UpdateReport> {
    update(
        selection,
        trust,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
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

fn edit_manifest(repo: &Path, from: &str, to: &str) {
    let path = repo.join("plugins/demo/plugin.json");
    let text = std::fs::read_to_string(&path).unwrap().replace(from, to);
    std::fs::write(path, text).unwrap();
}

// Covers: git install records its commit; update is a no-op until the ref
// moves, then replaces the package while keeping a user disable, and refuses
// a fetched package that renamed itself.
// Owner: plugin git install and update policy.
#[test]
fn git_install_then_update_follows_recorded_ref() {
    let env = env();
    let (repo, first) = plugin_repo("demo");
    let installed = install_git(
        &repo_source(repo.path(), Some("plugins/demo")),
        PluginScope::User,
        /* force */ false,
        env.project.path(),
        env.home.path(),
        Some(env.rho_home.path()),
    )
    .unwrap();
    assert_eq!(installed.package.origin, PluginOrigin::Git);
    assert_eq!(installed.commit, first);
    let manifest = env.home.path().join(".agents/plugins/demo/plugin.json");

    // A local install has no source to refetch.
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
    let error =
        run_update(&env, UpdateSelection::Named("local"), ProjectTrust::Trusted).unwrap_err();
    assert!(error.to_string().contains("not git"), "{error}");

    set_enabled(
        "demo",
        /* enabled */ false,
        env.project.path(),
        Some(env.home.path()),
        Some(env.rho_home.path()),
    )
    .unwrap();

    let outcomes = |report: super::UpdateReport| {
        report
            .results
            .into_iter()
            .map(|(target, result)| (target, result.ok()))
            .collect::<Vec<_>>()
    };
    let demo = UpdateTarget {
        name: "demo".to_string(),
        scope: PluginScope::User,
    };
    let report = run_update(&env, UpdateSelection::All, ProjectTrust::Trusted).unwrap();
    assert_eq!(
        outcomes(report),
        [(
            demo.clone(),
            Some(UpdateOutcome::UpToDate {
                commit: first.clone()
            })
        )]
    );

    edit_manifest(repo.path(), "1.2.3", "2.0.0");
    let second = commit_all(repo.path(), "bump");
    let report = run_update(&env, UpdateSelection::Named("demo"), ProjectTrust::Trusted).unwrap();
    assert_eq!(
        outcomes(report),
        [(
            demo.clone(),
            Some(UpdateOutcome::Updated {
                previous: first,
                current: second.clone(),
            })
        )]
    );
    assert!(std::fs::read_to_string(&manifest)
        .unwrap()
        .contains("2.0.0"));
    assert_eq!(demo_status(&env), PluginStatus::Disabled);

    edit_manifest(repo.path(), r#""name":"demo""#, r#""name":"renamed""#);
    commit_all(repo.path(), "rename");
    let report = run_update(&env, UpdateSelection::Named("demo"), ProjectTrust::Trusted).unwrap();
    assert_eq!(outcomes(report), [(demo, None)]);
    assert!(std::fs::read_to_string(&manifest)
        .unwrap()
        .contains("2.0.0"));
    let state = PluginStateStore::load(env.project.path(), Some(env.rho_home.path())).unwrap();
    assert_eq!(
        state.git_record(PluginScope::User, "demo").unwrap().commit,
        second
    );
}

// Covers: update-all keeps going past a failed package, and project sources
// from an untrusted workspace are never fetched.
// Owner: plugin git update selection.
#[test]
fn update_all_isolates_failures_and_skips_untrusted_project_sources() {
    let env = env();
    let (demo_repo, demo_commit) = plugin_repo("demo");
    let (other_repo, _) = plugin_repo("other");
    let (project_repo, _) = plugin_repo("proj");
    install_from(
        &env,
        &repo_source(demo_repo.path(), Some("plugins/demo")),
        PluginScope::User,
    );
    install_from(
        &env,
        &repo_source(other_repo.path(), Some("plugins/other")),
        PluginScope::User,
    );
    install_from(
        &env,
        &repo_source(project_repo.path(), Some("plugins/proj")),
        PluginScope::Project,
    );
    std::fs::remove_dir_all(env.home.path().join(".agents/plugins/other")).unwrap();

    let report = run_update(&env, UpdateSelection::All, ProjectTrust::Untrusted).unwrap();
    let results = report
        .results
        .into_iter()
        .map(|(target, result)| (target.name, result.ok()))
        .collect::<Vec<_>>();
    assert_eq!(
        results,
        [
            (
                "demo".to_string(),
                Some(UpdateOutcome::UpToDate {
                    commit: demo_commit
                })
            ),
            ("other".to_string(), None),
        ]
    );
    assert_eq!(report.untrusted, ["proj"]);

    assert!(run_update(
        &env,
        UpdateSelection::Named("proj"),
        ProjectTrust::Untrusted
    )
    .is_err());
    let trusted = run_update(&env, UpdateSelection::Named("proj"), ProjectTrust::Trusted).unwrap();
    assert!(trusted.results[0].1.is_ok());
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

    let missing = repo_source(&repo, Some("plugins/nope"));
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

    let root = repo_source(&repo, None);
    install_from(&env, &root, PluginScope::User);
    let installed = env.home.path().join(".agents/plugins/demo");
    assert!(installed.join("plugin.json").is_file());
    assert!(!installed.join(".git").exists());
    assert_eq!(demo_status(&env), PluginStatus::Loaded);
}

// Covers: from a home directory outside any repository, project state is the
// user state file under the default RHO_HOME; update must touch each package
// once and not call it untrusted. With a separate RHO_HOME the files differ
// and project records are still walked.
// Owner: plugin git update selection.
#[test]
fn update_from_home_directory_walks_each_state_file_once() {
    let cases = [
        // (separate RHO_HOME, install scope, untrusted skips, updated scope)
        (
            false,
            PluginScope::User,
            Vec::<&str>::new(),
            Some(PluginScope::User),
        ),
        (true, PluginScope::Project, vec!["demo"], None),
    ];
    for (separate_rho_home, scope, untrusted, updated) in cases {
        let home = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let rho_home = if separate_rho_home {
            other.path().to_path_buf()
        } else {
            home.path().join(".rho")
        };
        let (repo, commit) = plugin_repo("demo");
        install_git(
            &repo_source(repo.path(), Some("plugins/demo")),
            scope,
            /* force */ false,
            home.path(),
            home.path(),
            Some(&rho_home),
        )
        .unwrap();

        let report = update(
            UpdateSelection::All,
            ProjectTrust::Untrusted,
            home.path(),
            home.path(),
            Some(&rho_home),
        )
        .unwrap();
        let results = report
            .results
            .into_iter()
            .map(|(target, result)| (target.scope, result.ok()))
            .collect::<Vec<_>>();
        let expected = updated
            .map(|scope| {
                (
                    scope,
                    Some(UpdateOutcome::UpToDate {
                        commit: commit.clone(),
                    }),
                )
            })
            .into_iter()
            .collect::<Vec<_>>();
        assert_eq!(results, expected, "separate RHO_HOME: {separate_rho_home}");
        assert_eq!(
            report.untrusted, untrusted,
            "separate RHO_HOME: {separate_rho_home}"
        );
    }
}
