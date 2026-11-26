//! What the daemon's configuration sets up, read over its socket without a
//! session: `rho/mcp/list` and `rho/hooks/list`, for a client's settings
//! screen.

use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use rho_protocol::{
    methods::{HooksList, McpList},
    ClientEnvironment, ConfiguredMcpServer, HookStatus, HooksFileScope, HooksFileStatus,
    HooksListParams, McpListParams, McpServerSource, McpTransport,
};

use super::sessions::Sandbox;

const MCP_SERVERS: &str = r#"
[mcp.servers.fs]
transport = "stdio"
command = "fs-server"

[mcp.servers.broken]
transport = "stdio"
"#;

const USER_HOOKS: &str = r#"version = 1

[[hook]]
id = "log"
on = "after_tool_use"
command = ["logger"]
timeout = "1s"
"#;

const PROJECT_HOOKS: &str = r#"version = 1

[[hook]]
id = "fmt"
on = "after_tool_use"
command = ["./fmt"]
timeout = "1s"
"#;

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// `path` as the file system resolves it, so the daemon's canonical home
/// and the sandbox's name compare equal.
fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

// Covers: a client reads the MCP servers and hooks files the daemon's
// configuration sets up without opening a session: an invalid server is
// listed with its error rather than failing the request, and the project's
// hooks file loads only for a client whose environment trusts the project,
// as a session that client opened would load it.
// Owner: daemon configuration listing.
#[tokio::test]
async fn a_client_lists_the_configured_mcp_servers_and_hooks_without_a_session() {
    let sandbox = Sandbox::new();
    let config = std::fs::read_to_string(&sandbox.isolated.config_path).unwrap();
    write(
        &sandbox.isolated.config_path,
        &format!("{config}{MCP_SERVERS}"),
    );
    let user_hooks = sandbox.isolated.rho_home().join("hooks.toml");
    let project_hooks = sandbox.workspace.join(".rho/hooks.toml");
    write(&user_hooks, USER_HOOKS);
    write(&project_hooks, PROJECT_HOOKS);
    write(&sandbox.workspace.join("fmt"), "#!/bin/sh\n");
    let client = sandbox.start_daemon("fixed").await;

    let mcp = client
        .request::<McpList>(&McpListParams::new(
            Some(sandbox.workspace.clone()),
            ClientEnvironment::default(),
        ))
        .await
        .unwrap();
    assert_eq!(mcp.config_path, sandbox.shown_config_path());
    let broken = mcp
        .servers
        .iter()
        .find(|server| server.identity == "broken")
        .and_then(|server| server.error.clone())
        .expect("the invalid server is listed with its error");
    assert_eq!(
        mcp.servers,
        vec![
            ConfiguredMcpServer::new(
                "broken".into(),
                McpServerSource::Config,
                /*enabled*/ false,
                None,
                Some(broken),
            ),
            ConfiguredMcpServer::new(
                "fs".into(),
                McpServerSource::Config,
                /*enabled*/ true,
                Some(McpTransport::Stdio {
                    command: "fs-server".into(),
                    args: Vec::new(),
                }),
                None,
            ),
        ]
    );

    let trusting: ClientEnvironment = [("RHO_TRUST_PROJECT_HOOKS".to_owned(), "1".to_owned())]
        .into_iter()
        .collect();
    let mut listed = Vec::new();
    for environment in [ClientEnvironment::default(), trusting] {
        let hooks = client
            .request::<HooksList>(&HooksListParams::new(
                Some(sandbox.workspace.clone()),
                environment,
            ))
            .await
            .unwrap();
        listed.push(
            hooks
                .files
                .into_iter()
                .map(|file| {
                    let hooks = file
                        .hooks
                        .into_iter()
                        .map(|hook| (hook.id, hook.status))
                        .collect::<Vec<_>>();
                    (resolved(&file.path), file.scope, file.status, hooks)
                })
                .collect::<Vec<_>>(),
        );
    }
    let user = (
        resolved(&user_hooks),
        HooksFileScope::User,
        HooksFileStatus::Loaded,
        vec![("user:log".to_owned(), HookStatus::Active)],
    );
    let project = |status, hook_status| {
        (
            resolved(&project_hooks),
            HooksFileScope::Project,
            status,
            vec![("project:fmt".to_owned(), hook_status)],
        )
    };
    assert_eq!(
        listed,
        vec![
            vec![
                user.clone(),
                project(
                    HooksFileStatus::Untrusted { error: None },
                    HookStatus::Untrusted
                ),
            ],
            vec![user, project(HooksFileStatus::Loaded, HookStatus::Active)],
        ]
    );
}
