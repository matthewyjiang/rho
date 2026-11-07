//! The processes a daemon session's tools start, through the real binary:
//! the daemon's stop ends them, as closing a session ends them in every
//! rho host (#1661).

use std::path::Path;

use pretty_assertions::assert_eq;
use rho_protocol::{methods::SessionCancel, JobState, SessionCancelParams};
use serde_json::json;

use super::sessions::{new_session, prompt_tool_call, settled_job, Sandbox};

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
    let (_daemon, client) = sandbox.start_daemon("tool-call").await;
    let running_turn = new_session(&client, &sandbox.workspace).await;
    let bash = json!({
        "id": "foreground",
        "name": "bash",
        "arguments": {"command": sleeper("foreground.pid")},
    });
    prompt_tool_call(&client, &running_turn, &bash).await;
    let cancelled = new_session(&client, &sandbox.workspace).await;
    let process = json!({
        "id": "background",
        "name": "process",
        "arguments": {"action": "start", "command": sleeper("background.pid")},
    });
    prompt_tool_call(&client, &cancelled, &process).await;
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
    assert!(stop.success(), "log:\n{}", sandbox.daemon_log());
    sandbox
        .poll_until("the processes to end", async || {
            (!pids.into_iter().any(running)).then_some(())
        })
        .await;

    assert_eq!(
        (cancelled_state, before_stop),
        (JobState::Cancelled, [true, true]),
        "log:\n{}",
        sandbox.daemon_log()
    );
}
