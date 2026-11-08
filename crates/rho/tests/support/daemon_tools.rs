//! The processes, MCP servers, and hooks of a daemon session, through the
//! real binary: they run with the client's environment (#1659), and the
//! daemon's stop ends the processes its sessions started, as closing a
//! session ends them in every rho host (#1661).

use std::path::Path;

use pretty_assertions::assert_eq;
use rho_protocol::{methods::SessionCancel, JobState, SessionCancelParams};
use serde_json::json;

use super::sessions::{settled_job, Sandbox};

/// A command whose shell starts a child that sleeps, records the child's
/// pid in `file` in the workspace, and waits for it. The child is the
/// tool's grandchild, so only ending the tool's whole process group ends
/// it. It sleeps far past the test's deadline, so it never ends on its own
/// while the test waits.
fn sleeper(file: &str) -> String {
    format!("sleep 600 & echo $! > {file} && wait")
}

/// The pid a [`sleeper`] recorded in `file` under `workspace`, once it has.
fn recorded_pid(workspace: &Path, file: &str) -> Option<libc::pid_t> {
    std::fs::read_to_string(workspace.join(file))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Whether process `pid` still runs. A killed grandchild is reparented, and
/// a container's PID 1 may never reap it, so on Linux a zombie counts as
/// ended, as in `process_tree::process_is_running`, which tests outside
/// the crate cannot call.
fn running(pid: libc::pid_t) -> bool {
    // SAFETY: `kill` takes plain integers; signal 0 checks that `pid`
    // exists without sending anything.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        stat.rsplit_once(") ")
            .is_none_or(|(_, fields)| !fields.starts_with("Z "))
    }
    #[cfg(not(target_os = "linux"))]
    true
}

/// Kills the process groups of the sleepers still running when dropped, so
/// a failed test, whose daemon the harness kills without a stop, leaves
/// none behind. Each sleeper's group is its tool's.
struct Sleepers(Vec<libc::pid_t>);

impl Drop for Sleepers {
    fn drop(&mut self) {
        for &pid in self.0.iter().filter(|&&pid| running(pid)) {
            // SAFETY: plain integer calls; a pid that ended meanwhile makes
            // `getpgid` fail, and nothing is killed.
            unsafe {
                let group = libc::getpgid(pid);
                if group > 0 {
                    libc::killpg(group, libc::SIGKILL);
                }
            }
        }
    }
}

// Covers: `rho daemon stop` ends the processes its sessions' tools started,
// children included: the command a turn under way runs, and a background
// process that a turn started and that outlived the turn's cancel (#1661).
// Owner: daemon stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_daemons_stop_ends_the_processes_its_sessions_started() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let (mut daemon, client) = sandbox.spawn_daemon(sandbox.rho("tool-call"), &[]).await;
    let running_turn = sandbox.new_session(&client).await;
    let bash = json!({
        "id": "foreground",
        "name": "bash",
        "arguments": {"command": sleeper("foreground.pid")},
    });
    sandbox
        .prompt_tool_call(&client, &running_turn, &bash)
        .await;
    let cancelled = sandbox.new_session(&client).await;
    let process = json!({
        "id": "background",
        "name": "process",
        "arguments": {"action": "start", "command": sleeper("background.pid")},
    });
    sandbox
        .prompt_tool_call(&client, &cancelled, &process)
        .await;
    let pids = sandbox
        .poll_until("both processes to start", async || {
            Some([
                recorded_pid(&sandbox.workspace, "foreground.pid")?,
                recorded_pid(&sandbox.workspace, "background.pid")?,
            ])
        })
        .await;
    let _leftovers = Sleepers(pids.to_vec());
    // A daemon turn waits for the background processes it started, so only
    // its cancel lets one outlive it.
    client
        .request::<SessionCancel>(&SessionCancelParams {
            session: cancelled.clone(),
        })
        .await
        .unwrap();
    let cancelled_state = settled_job(&sandbox, &client, &cancelled).await.state;
    let before_stop = pids.map(running);

    let stop = sandbox
        .rho("tool-call")
        .args(["daemon", "stop"])
        .status()
        .unwrap();
    assert!(stop.success(), "log:\n{}", sandbox.isolated.daemon_log());
    daemon.wait().unwrap();
    sandbox
        .poll_until("the processes to end", async || {
            (!pids.into_iter().any(running)).then_some(())
        })
        .await;

    assert_eq!(
        (cancelled_state, before_stop),
        (JobState::Cancelled, [true, true]),
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a daemon session's tool processes, the shell tool's and the
// `process` tool's, run with the prompting client's environment: its `PATH`
// leading after login init, without its provider credentials, and with none
// of the daemon's own; the next prompt's replaces it; neither the session
// file nor the daemon log holds its values (#1659).
// Owner: daemon session environment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sessions_tools_run_with_the_prompting_clients_environment() {
    let sandbox = Sandbox::new();
    sandbox.set_permission_mode("bypass");
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session(&client).await;
    // Login init that resets `PATH`, as some distributions' does, in the
    // home the sandbox's clients send.
    std::fs::write(
        sandbox.isolated.home.join(".bash_profile"),
        "export PATH=/usr/bin:/bin\n",
    )
    .unwrap();
    // The daemon has the fixture's mode set; no client sends it.
    let record = "printf '%s|%s|%s|%s' \"${PATH%%:*}\" \"$PROBE\" \"${XAI_API_KEY-unset}\" \"${RHO_AUTOMATION_TEST_MODE-unset}\" > env.txt";
    let calls = [
        (
            "probe-1659-bash",
            json!({"id": "bash", "name": "bash", "arguments": {"command": record}}),
        ),
        (
            "probe-1659-process",
            json!({
                "id": "process",
                "name": "process",
                "arguments": {"action": "start", "command": record},
            }),
        ),
    ];
    let mut seen = Vec::new();
    for (probe, call) in calls {
        let path = format!("/rho-client-1659:{}", std::env::var("PATH").unwrap());
        let environment = sandbox
            .client_environment()
            .into_iter()
            .chain([
                ("PATH".to_owned(), path),
                ("PROBE".to_owned(), probe.to_owned()),
                ("XAI_API_KEY".to_owned(), "key-1659".to_owned()),
            ])
            .collect();
        sandbox
            .prompt_tool_call_with_environment(&client, &session, &call, environment)
            .await;
        // The turn waits for a background process it started.
        settled_job(&sandbox, &client, &session).await;
        seen.push(std::fs::read_to_string(sandbox.workspace.join("env.txt")).unwrap());
    }
    let saved = serde_json::to_string(&sandbox.session_records(&session)).unwrap();
    let log = sandbox.isolated.daemon_log();

    assert_eq!(
        seen,
        [
            "/rho-client-1659|probe-1659-bash|unset|unset",
            "/rho-client-1659|probe-1659-process|unset|unset"
        ],
        "log:\n{log}"
    );
    assert_eq!(
        ["probe-1659", "key-1659", "rho-client-1659"]
            .map(|value| saved.contains(value) || log.contains(value)),
        [false, false, false]
    );
}

/// A client environment: the sandbox's, with `PATH` led by `bin` and the
/// `extra` variables.
fn client_environment_with_bin(
    sandbox: &Sandbox,
    bin: &Path,
    extra: &[(&str, &str)],
) -> rho_protocol::ClientEnvironment {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    sandbox
        .client_environment()
        .into_iter()
        .chain([("PATH".to_owned(), path)])
        .chain(
            extra
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
        )
        .collect()
}

// Covers: a daemon session's stdio MCP server is found on the opening
// client's `PATH` and takes its base set and `env_from_env` from that
// client's environment; a hook takes its base set and `env` names from the
// environment of the prompt it runs in (#1659).
// Owner: daemon session environment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sessions_mcp_servers_and_hooks_see_its_clients_environment() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new();
    let root = sandbox.workspace.parent().unwrap().to_path_buf();
    // Only clients have this directory on their `PATH`.
    let bin = root.join("client-bin-1659");
    std::fs::create_dir_all(&bin).unwrap();
    let record = |file: &str, value: &str| {
        format!(
            "printf '%s|%s\\n' \"${{PATH%%:*}}\" \"{value}\" >> {}",
            root.join(file).display()
        )
    };
    // Records what it was started with, then exits: the session goes on
    // without its tools.
    let server = bin.join("probe-mcp-1659");
    std::fs::write(
        &server,
        format!("#!/bin/sh\n{}\n", record("mcp.txt", "${TOKEN-unset}")),
    )
    .unwrap();
    std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
    sandbox.set_config(
        "bypass",
        r#"
[mcp.servers.probe]
transport = "stdio"
command = "probe-mcp-1659"
env_from_env = { TOKEN = "PROBE_TOKEN" }
"#,
    );
    let hook = record("hook.txt", "${PROBE-unset}")
        + " && printf '{\"version\": 1, \"decision\": \"continue\"}'";
    std::fs::write(
        sandbox.isolated.rho_home().join("hooks.toml"),
        format!(
            r#"version = 1

[[hook]]
id = "probe"
on = "before_tool_use"
tools = ["bash"]
command = ["/bin/sh", "-c", {hook:?}]
env = ["PROBE"]
timeout = "10s"
"#
        ),
    )
    .unwrap();
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox
        .new_session_with_environment(
            &client,
            client_environment_with_bin(&sandbox, &bin, &[("PROBE_TOKEN", "token-1659")]),
        )
        .await;
    let call = json!({"id": "bash", "name": "bash", "arguments": {"command": "true"}});
    for probe in ["first-1659", "second-1659"] {
        let environment = client_environment_with_bin(&sandbox, &bin, &[("PROBE", probe)]);
        sandbox
            .prompt_tool_call_with_environment(&client, &session, &call, environment)
            .await;
        settled_job(&sandbox, &client, &session).await;
    }
    let read = |file: &str| std::fs::read_to_string(root.join(file)).unwrap_or_default();
    let bin = bin.display();

    assert_eq!(
        [read("mcp.txt"), read("hook.txt")],
        [
            format!("{bin}|token-1659\n"),
            format!("{bin}|first-1659\n{bin}|second-1659\n"),
        ],
        "log:\n{}",
        sandbox.isolated.daemon_log()
    );
}

// Covers: a daemon session's CLI agent (Claude Code here) is found on the
// prompting client's `PATH` and runs with that client's whole environment,
// credentials included, since it signs in itself; a workflow run the
// session starts finds its command's executable and `#!/usr/bin/env`
// interpreter on that `PATH` and runs it with the client's environment
// minus credentials (#1659).
// Owner: daemon session environment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sessions_cli_agents_and_workflow_commands_see_its_clients_environment() {
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new();
    let root = sandbox.workspace.parent().unwrap().to_path_buf();
    // Only clients have this directory on their `PATH`.
    let bin = root.join("client-bin-1659");
    std::fs::create_dir_all(&bin).unwrap();
    // Installs an executable script with these whole contents, `#!` line
    // included.
    let install = |name: &str, contents: String| {
        let path = bin.join(name);
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    let record = |file: &str| {
        format!(
            "printf '%s|%s|%s\\n' \"${{PATH%%:*}}\" \"${{PROBE-unset}}\" \"${{XAI_API_KEY-unset}}\" >> {}",
            root.join(file).display()
        )
    };
    // Answers the adapter's sign-in probe, records the agent's launch, then
    // fails it: the run's outcome is not what this covers.
    install(
        "claude-agent-acp",
        format!(
            "#!/bin/sh\nif [ \"${{1-}}\" = \"--cli\" ]; then printf '%s\\n' '{{\"loggedIn\":true}}'; exit 0; fi\n{}\nexit 1\n",
            record("cli.txt")
        ),
    );
    // The command runs through an interpreter only clients have, as a
    // `#!/usr/bin/env node` script does with mise's `node` (a symlink, as
    // mise's is: a copy of macOS's `/bin/sh` trampoline does not run).
    std::os::unix::fs::symlink("/bin/sh", bin.join("sh-1659")).unwrap();
    install(
        "probe-1659",
        format!("#!/usr/bin/env sh-1659\n{}\n", record("command.txt")),
    );
    // A user agent, in the sandbox's rho home.
    std::fs::create_dir_all(sandbox.isolated.rho_home().join("agents")).unwrap();
    std::fs::write(
        sandbox.isolated.rho_home().join("agents/claude-env.md"),
        "---\nid: claude-env\ndescription: Claude Code agent for the environment test\nruntime: claude-cli\nprompt: replace\n---\nWork.\n",
    )
    .unwrap();
    std::fs::create_dir_all(sandbox.workspace.join("wf")).unwrap();
    std::fs::write(
        sandbox.workspace.join("wf/main.star"),
        r#"def build(inputs):
    return workflow(name = "env-run", nodes = [
        command(
            name = "probe",
            argv = ["probe-1659"],
            cwd = ".",
            timeout_seconds = 600,
            max_output_bytes = 1000,
        ),
    ])

WORKFLOW = define(inputs = {}, build = build)
"#,
    )
    .unwrap();
    // Claude Code runs only in modes whose permissions it can enforce.
    sandbox.set_permission_mode("bypass");
    let client = sandbox.start_daemon("tool-call").await;
    let session = sandbox.new_session(&client).await;
    let mut calls = vec![(
        "cli.txt",
        "cli-1659",
        json!({"id": "delegate", "name": "agent", "arguments": {"agent_id": "claude-env", "prompt": "Work."}}),
        "key-1659",
    )];
    // Frozen workflow commands launch only where the OS adapter can start
    // them from verified handles (`tools/process/exact.rs`).
    if cfg!(any(target_os = "linux", target_os = "android")) {
        calls.push((
            "command.txt",
            "workflow-1659",
            json!({"id": "start", "name": "workflow", "arguments": {"action": "start", "file": "wf/main.star"}}),
            "unset",
        ));
    }
    let mut seen = Vec::new();
    let mut expected = Vec::new();
    for (file, probe, call, key) in calls {
        let environment = client_environment_with_bin(
            &sandbox,
            &bin,
            &[("PROBE", probe), ("XAI_API_KEY", "key-1659")],
        );
        sandbox
            .prompt_tool_call_with_environment(&client, &session, &call, environment)
            .await;
        settled_job(&sandbox, &client, &session).await;
        // A workflow run outlives the turn that started it.
        seen.push(
            sandbox
                .poll_until(file, async || std::fs::read_to_string(root.join(file)).ok())
                .await,
        );
        expected.push(format!("{}|{probe}|{key}\n", bin.display()));
    }

    assert_eq!(seen, expected, "log:\n{}", sandbox.isolated.daemon_log());
}
