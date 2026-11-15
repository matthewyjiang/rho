use std::{path::Path, time::Duration};

use pretty_assertions::assert_eq;
use tokio::sync::watch;

use super::*;
use crate::{
    app::{agent_executor::AgentRunHandle, subagent_pool::SubagentPool},
    subagent::{RunState, RunStatus},
    tools::process::{KeepWarm, ProcessLimits, ProcessRole},
};

/// A command that outlives every wait here, in the shell of either platform.
const LONG_RUNNING_COMMAND: &str = "sleep 300";

/// Bounds each wait on a real process exit; a hang fails.
const TEST_WAIT: Duration = Duration::from_secs(10);

const SESSION: &str = "parent";

/// What one session started.
#[derive(Clone, Copy, Debug)]
enum Work {
    Nothing,
    LiveTask,
    ExitedTask,
    LiveService,
    ExitedService,
    RunningSubagent,
    FinishedSubagent,
}

const TASK: ProcessRole = ProcessRole::Task {
    keep_warm: KeepWarm::Off,
};

/// What [`AwaitedWork`] says of a session's work.
#[derive(Debug, PartialEq)]
struct Awaited {
    outstanding: bool,
    has_result: bool,
    running: bool,
}

/// A session that started `work`, its daemon turns delivering its
/// subagents; its process manager; and what keeps a running subagent
/// running.
async fn started(
    work: Work,
    root: &Path,
) -> (AwaitedWork, ProcessManager, Option<watch::Sender<bool>>) {
    let processes = ProcessManager::new(ProcessLimits::default());
    let subagents = SubagentManager::new(
        crate::config::Config::default(),
        root.join("config.toml"),
        root.to_path_buf(),
        SubagentPool::Own,
    );
    let mut running = None;
    match work {
        Work::Nothing => {}
        Work::LiveTask => {
            start(&processes, root, TASK).await;
        }
        Work::LiveService => {
            start(&processes, root, ProcessRole::Service).await;
        }
        Work::ExitedTask => exit(&processes, &start(&processes, root, TASK).await).await,
        Work::ExitedService => {
            let id = start(&processes, root, ProcessRole::Service).await;
            exit(&processes, &id).await;
        }
        Work::RunningSubagent => {
            let (_status, status) = watch::channel(RunStatus {
                state: RunState::Running,
                ..Default::default()
            });
            let (completion, completion_rx) = watch::channel(false);
            subagents.insert_handle_for_test(
                "abc123",
                SESSION,
                AgentRunHandle::controlled_for_test(
                    status,
                    completion_rx,
                    rho_tools::cancellation::RunCancellation::new(),
                ),
            );
            running = Some(completion);
        }
        Work::FinishedSubagent => subagents.insert_completed_status_for_test(
            "abc123",
            SESSION,
            RunStatus {
                state: RunState::Ok,
                ..Default::default()
            },
        ),
    }
    let awaited = AwaitedWork::new(
        BackgroundWait::HostTurns,
        SESSION.into(),
        Some(processes.clone()),
        WorkflowRunTracker::new(),
        Some(subagents),
    )
    .unwrap();
    (awaited, processes, running)
}

/// Starts a long-running process as `role` in `root`; returns its id.
async fn start(processes: &ProcessManager, root: &Path, role: ProcessRole) -> String {
    processes
        .start_as(LONG_RUNNING_COMMAND.into(), root, None, role)
        .await
        .unwrap()
        .process_id
}

/// Stops process `id` and returns once it exited.
async fn exit(processes: &ProcessManager, id: &str) {
    let exited = processes.notified_owned();
    processes.stop(id, Duration::ZERO).await.unwrap();
    tokio::time::timeout(TEST_WAIT, exited).await.unwrap();
}

// Covers: a task or subagent the session started is awaited until a turn
// takes its result, and is a result once it finished; a service, live or
// exited, is neither, so it never wakes the idle session nor keeps it from
// settling; and work counts as running exactly while it may still give a
// result, which a waker that gave up relies on. Owner: daemon awaited work.
#[tokio::test]
async fn tasks_and_subagents_are_awaited_and_services_are_not() {
    let awaited = |outstanding, has_result, running| Awaited {
        outstanding,
        has_result,
        running,
    };
    let cases = [
        (Work::Nothing, awaited(false, false, false)),
        (Work::LiveTask, awaited(true, false, true)),
        (Work::ExitedTask, awaited(true, true, false)),
        (Work::LiveService, awaited(false, false, false)),
        (Work::ExitedService, awaited(false, false, false)),
        (Work::RunningSubagent, awaited(true, false, true)),
        (Work::FinishedSubagent, awaited(true, true, false)),
    ];
    for (work, expected) in cases {
        let root = tempfile::tempdir().unwrap();
        let (work_of, processes, _running) = started(work, root.path()).await;
        assert_eq!(
            Awaited {
                outstanding: work_of.outstanding(),
                has_result: work_of.has_result(),
                running: work_of.running(),
            },
            expected,
            "{work:?}"
        );
        processes.shutdown().await;
    }
}
