//! Managing agent plugins through the daemon (`rho/plugins/*`), through the
//! real binary and its socket, as rho-ui's Settings → Plugins page does:
//! what is listed for a trusting and an untrusted client, installing from a
//! local directory, a link, and a git repository, refusing a conflict
//! without `force`, saving enable state, updating, and removing.

use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{PluginsInstall, PluginsList, PluginsRemove, PluginsSetEnabled, PluginsUpdate},
    Client, ClientEnvironment, ClientError, ErrorCode, Plugin, PluginConflict, PluginInstallSource,
    PluginScope, PluginSource, PluginStatus, PluginTarget, PluginUpdate, PluginUpdateOutcome,
    PluginsInstallParams, PluginsListParams, PluginsRemoveParams, PluginsSetEnabledParams,
    PluginsUpdateParams, PluginsUpdateResponse, RpcError,
};

use super::sessions::Sandbox;

const SCHEMA: &str = "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";

/// A plugin package named `name` at `dir`, at `version`, with one skill.
fn write_package(dir: &Path, name: &str, version: &str) {
    std::fs::create_dir_all(dir.join("skills/hello")).unwrap();
    std::fs::write(
        dir.join("plugin.json"),
        format!(r#"{{"$schema":"{SCHEMA}","name":"{name}","version":"{version}"}}"#),
    )
    .unwrap();
    std::fs::write(
        dir.join("skills/hello/SKILL.md"),
        "---\nname: hello\ndescription: greets\n---\nbody\n",
    )
    .unwrap();
}

fn trusting() -> ClientEnvironment {
    [("RHO_TRUST_PROJECT_PLUGINS".to_owned(), "1".to_owned())]
        .into_iter()
        .collect()
}

fn rpc_error(error: ClientError) -> RpcError {
    match error {
        ClientError::Rpc { error, .. } => error,
        error => panic!("failed outside the protocol: {error}"),
    }
}

/// `path` as the file system resolves it, so the daemon's paths and the
/// sandbox's compare equal.
fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

/// What a row of the plugins page shows, paths resolved.
fn row(plugin: &Plugin) -> (String, PluginScope, PluginStatus, bool, PluginSource) {
    let source = match &plugin.source {
        PluginSource::Link { target } => PluginSource::Link {
            target: resolved(target),
        },
        source => source.clone(),
    };
    (
        plugin.name.clone(),
        plugin.scope,
        plugin.status,
        plugin.enabled,
        source,
    )
}

async fn list(client: &Client, sandbox: &Sandbox, environment: ClientEnvironment) -> Vec<Plugin> {
    client
        .request::<PluginsList>(&PluginsListParams::new(
            Some(sandbox.workspace.clone()),
            environment,
        ))
        .await
        .unwrap()
        .plugins
}

async fn install(
    client: &Client,
    source: PluginInstallSource,
    scope: PluginScope,
    force: bool,
    workspace: Option<PathBuf>,
) -> Result<Plugin, RpcError> {
    client
        .request::<PluginsInstall>(&PluginsInstallParams::new(
            source,
            scope,
            force,
            workspace,
            ClientEnvironment::default(),
        ))
        .await
        .map(|response| response.plugin)
        .map_err(rpc_error)
}

// Covers: a client lists the user's and the workspace project's plugins
// without a session, a project plugin staying inactive for a client that
// does not trust the project and a broken `plugin.json` listed with its
// error; installs a local package and links another, a second install of
// a name refused without `force` with the plugin there named; disables a
// plugin so a later list still shows it disabled; and removes one by
// scope, leaving a same-named project plugin alone.
// Owner: daemon plugin management.
#[tokio::test]
async fn a_client_manages_local_plugins_without_a_session() {
    let sandbox = Sandbox::new();
    let sources = sandbox.isolated.path().join("sources");
    write_package(&sources.join("demo"), "demo", "1.0.0");
    write_package(&sources.join("linked"), "linked", "1.0.0");
    let project_plugins = sandbox.workspace.join(".agents/plugins");
    write_package(&project_plugins.join("demo"), "demo", "0.1.0");
    std::fs::create_dir_all(project_plugins.join("broken")).unwrap();
    std::fs::write(project_plugins.join("broken/plugin.json"), "{").unwrap();
    let client = sandbox.start_daemon("fixed").await;

    let installed = install(
        &client,
        PluginInstallSource::Path {
            path: sources.join("demo"),
        },
        PluginScope::User,
        false,
        None,
    )
    .await
    .unwrap();
    let user_demo = sandbox.isolated.home.join(".agents/plugins/demo");
    assert_eq!(
        (
            row(&installed),
            resolved(&installed.path),
            installed.skills.clone()
        ),
        (
            (
                "demo".to_owned(),
                PluginScope::User,
                PluginStatus::Loaded,
                true,
                PluginSource::Install
            ),
            resolved(&user_demo),
            vec!["hello".to_owned()]
        )
    );

    let conflict = install(
        &client,
        PluginInstallSource::Path {
            path: sources.join("demo"),
        },
        PluginScope::User,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(conflict.error_code(), Some(ErrorCode::PluginExists));
    let conflict: PluginConflict = serde_json::from_value(conflict.data.unwrap()).unwrap();
    assert_eq!(
        (conflict.name, resolved(&conflict.path)),
        ("demo".to_owned(), resolved(&user_demo))
    );
    install(
        &client,
        PluginInstallSource::Path {
            path: sources.join("demo"),
        },
        PluginScope::User,
        true,
        None,
    )
    .await
    .unwrap();
    let linked = install(
        &client,
        PluginInstallSource::Link {
            path: sources.join("linked"),
        },
        PluginScope::Project,
        false,
        Some(sandbox.workspace.clone()),
    )
    .await
    .unwrap();
    assert_eq!(
        row(&linked),
        (
            "linked".to_owned(),
            PluginScope::Project,
            PluginStatus::Untrusted,
            true,
            PluginSource::Link {
                target: resolved(&sources.join("linked")),
            }
        )
    );

    let mut rows = Vec::new();
    for environment in [ClientEnvironment::default(), trusting()] {
        rows.push(
            list(&client, &sandbox, environment)
                .await
                .iter()
                .map(row)
                .collect::<Vec<_>>(),
        );
    }
    let project = |name: &str, status| {
        (
            name.to_owned(),
            PluginScope::Project,
            status,
            true,
            match name {
                "linked" => PluginSource::Link {
                    target: resolved(&sources.join("linked")),
                },
                _ => PluginSource::Local,
            },
        )
    };
    let user_demo_row = |status| {
        (
            "demo".to_owned(),
            PluginScope::User,
            status,
            true,
            PluginSource::Install,
        )
    };
    assert_eq!(
        rows,
        vec![
            // Untrusted, the project's `demo` leaves the user's loaded.
            vec![
                project("broken", PluginStatus::Rejected),
                project("demo", PluginStatus::Untrusted),
                project("linked", PluginStatus::Untrusted),
                user_demo_row(PluginStatus::Loaded),
            ],
            vec![
                project("broken", PluginStatus::Rejected),
                project("demo", PluginStatus::Loaded),
                project("linked", PluginStatus::Loaded),
                user_demo_row(PluginStatus::Shadowed),
            ],
        ]
    );
    let broken = &list(&client, &sandbox, ClientEnvironment::default()).await[0];
    assert!(
        broken.error.is_some() && broken.problems.is_empty(),
        "{broken:?}"
    );

    let disabled = client
        .request::<PluginsSetEnabled>(&PluginsSetEnabledParams::new(
            PluginTarget::new("demo".into(), PluginScope::User),
            false,
            Some(sandbox.workspace.clone()),
            ClientEnvironment::default(),
        ))
        .await
        .unwrap()
        .plugin;
    let state_path = sandbox.isolated.rho_home().join("plugins.toml");
    assert_eq!(
        (
            disabled.status,
            disabled.enabled,
            resolved(&disabled.state_path)
        ),
        (PluginStatus::Disabled, false, resolved(&state_path))
    );
    let listed = list(&client, &sandbox, ClientEnvironment::default()).await;
    assert_eq!(
        listed.last().map(row),
        Some((
            "demo".to_owned(),
            PluginScope::User,
            PluginStatus::Disabled,
            false,
            PluginSource::Install
        ))
    );

    let remove = async |scope| {
        let params = PluginsRemoveParams::new(
            PluginTarget::new("demo".into(), scope),
            Some(sandbox.workspace.clone()),
        );
        client.request::<PluginsRemove>(&params).await
    };
    let removed = remove(PluginScope::User).await.unwrap();
    assert_eq!(removed.path.file_name(), user_demo.file_name());
    assert!(!user_demo.exists());
    assert!(project_plugins.join("demo").exists());
    let again = rpc_error(remove(PluginScope::User).await.unwrap_err());
    assert_eq!(again.error_code(), Some(ErrorCode::PluginNotFound));

    let no_workspace = install(
        &client,
        PluginInstallSource::Path {
            path: sources.join("demo"),
        },
        PluginScope::Project,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(no_workspace.error_code(), Some(ErrorCode::InvalidParams));
}

/// Run git in `dir` with an identity that ignores ambient signing config.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
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
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit_all(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "--quiet", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

// Covers: a client installs a plugin from a git repository on the daemon's
// machine, into the user's root and the project's, and updates it: nothing
// moves until the repository does, a project plugin is fetched only for a
// client that trusts the project and a named update of it is refused
// otherwise, and a repository git cannot fetch is told apart from other
// failures.
// Owner: daemon plugin management.
#[tokio::test]
async fn a_client_installs_and_updates_a_git_plugin() {
    let sandbox = Sandbox::new();
    let repo = sandbox.isolated.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    write_package(&repo, "demo", "1.0.0");
    let first = commit_all(&repo, "init");
    let url = url::Url::from_directory_path(&repo).unwrap().to_string();
    let client = sandbox.start_daemon("fixed").await;

    let git_source = || PluginInstallSource::Git { url: url.clone() };
    let installed = install(&client, git_source(), PluginScope::User, false, None)
        .await
        .unwrap();
    assert_eq!(
        installed.source,
        PluginSource::Git {
            url: url.clone(),
            git_ref: None,
            path: None,
            commit: first.clone(),
        }
    );
    install(
        &client,
        git_source(),
        PluginScope::Project,
        false,
        Some(sandbox.workspace.clone()),
    )
    .await
    .unwrap();

    let update = async |plugin: Option<PluginScope>, environment| {
        let params = PluginsUpdateParams::new(
            plugin.map(|scope| PluginTarget::new("demo".into(), scope)),
            Some(sandbox.workspace.clone()),
            environment,
        );
        client.request::<PluginsUpdate>(&params).await
    };
    let target = |scope| PluginTarget::new("demo".into(), scope);
    assert_eq!(
        update(None, ClientEnvironment::default()).await.unwrap(),
        PluginsUpdateResponse::new(
            vec![PluginUpdate::new(
                target(PluginScope::User),
                PluginUpdateOutcome::UpToDate {
                    commit: first.clone()
                },
            )],
            vec!["demo".to_owned()],
        )
    );

    write_package(&repo, "demo", "2.0.0");
    let second = commit_all(&repo, "bump");
    let untrusted = rpc_error(
        update(Some(PluginScope::Project), ClientEnvironment::default())
            .await
            .unwrap_err(),
    );
    assert_eq!(untrusted.error_code(), Some(ErrorCode::PluginUntrusted));
    assert_eq!(
        update(Some(PluginScope::Project), trusting())
            .await
            .unwrap(),
        PluginsUpdateResponse::new(
            vec![PluginUpdate::new(
                target(PluginScope::Project),
                PluginUpdateOutcome::Updated {
                    previous: first,
                    current: second,
                },
            )],
            Vec::new(),
        )
    );
    let versions = list(&client, &sandbox, trusting())
        .await
        .into_iter()
        .map(|plugin| (plugin.scope, plugin.version))
        .collect::<Vec<_>>();
    assert_eq!(
        versions,
        vec![
            (PluginScope::Project, Some("2.0.0".to_owned())),
            (PluginScope::User, Some("1.0.0".to_owned())),
        ]
    );

    let missing = url::Url::from_directory_path(sandbox.isolated.path().join("no-such-repo"))
        .unwrap()
        .to_string();
    let unfetched = install(
        &client,
        PluginInstallSource::Git { url: missing },
        PluginScope::User,
        true,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(unfetched.error_code(), Some(ErrorCode::PluginFetchFailed));
}
