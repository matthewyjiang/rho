use super::types::{terminal, Stream};
use super::*;
use rho_tools::tool::{Tool, ToolContext, ToolError};
use serde_json::json;
use std::{path::PathBuf, time::Duration};

#[cfg(unix)]
const LONG_RUNNING_COMMAND: &str = "sleep 300";
#[cfg(windows)]
const LONG_RUNNING_COMMAND: &str = "Start-Sleep -Seconds 300";
#[cfg(unix)]
const STDIN_CLOSED_COMMAND: &str = "read input || printf closed";
#[cfg(windows)]
const STDIN_CLOSED_COMMAND: &str = "$input = [Console]::In.ReadToEnd(); if ($input.Length -eq 0) { [Console]::Out.Write('closed') }";
#[cfg(unix)]
const MIXED_OUTPUT_COMMAND: &str = "printf out; printf err >&2";
#[cfg(windows)]
const MIXED_OUTPUT_COMMAND: &str = "[Console]::Out.Write('out'); [Console]::Error.Write('err')";
#[cfg(unix)]
const LIMITED_OUTPUT_COMMAND: &str = "printf abc; printf def";
#[cfg(windows)]
const LIMITED_OUTPUT_COMMAND: &str = "[Console]::Out.Write('abcdef')";
#[cfg(unix)]
const DELAYED_OUTPUT_COMMAND: &str = "sleep 0.05; printf wake";
#[cfg(windows)]
const DELAYED_OUTPUT_COMMAND: &str = "Start-Sleep -Milliseconds 50; [Console]::Out.Write('wake')";
#[cfg(unix)]
const LARGE_OUTPUT_COMMAND: &str = "head -c 1000000 /dev/zero | tr '\\0' x";
#[cfg(windows)]
const LARGE_OUTPUT_COMMAND: &str = "[Console]::Out.Write('x' * 1000000)";
#[cfg(unix)]
const SUCCESS_COMMAND: &str = "true";
#[cfg(windows)]
const SUCCESS_COMMAND: &str = "exit 0";
#[cfg(unix)]
const OUTPUT_THEN_SLEEP_COMMAND: &str = "printf hello; sleep 300";
#[cfg(windows)]
const OUTPUT_THEN_SLEEP_COMMAND: &str = "[Console]::Out.Write('hello'); Start-Sleep -Seconds 300";

async fn eventually(manager: &ProcessManager, id: &str) -> Snapshot {
    let mut cursor = 0;
    loop {
        let snapshot = manager
            .poll(id, Some(cursor), Duration::from_secs(2))
            .await
            .unwrap();
        cursor = snapshot.next_cursor;
        if terminal(snapshot.state) {
            return snapshot;
        }
    }
}

fn tool_context() -> ToolContext {
    ToolContext {
        cwd: PathBuf::from("."),
        max_output_bytes: 1024 * 1024,
    }
}

fn header_field(content: &str, key: &str) -> String {
    let prefix = format!("{key}: ");
    content
        .lines()
        .find_map(|line| line.strip_prefix(&prefix).map(str::to_string))
        .unwrap_or_else(|| panic!("missing {key} in:\n{content}"))
}
#[tokio::test]
async fn process_tool_dispatches_start_poll_and_stop() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let tool = Process::new(manager.clone());
    let started = tool
        .call(
            json!({"action": "start", "command": LONG_RUNNING_COMMAND}),
            tool_context(),
            "start-call".into(),
        )
        .await
        .unwrap();
    let process_id = header_field(&started.content, "process_id");

    let polled = tool
        .call(
            json!({"action": "poll", "process_id": process_id}),
            tool_context(),
            "poll-call".into(),
        )
        .await
        .unwrap();
    assert_eq!(header_field(&polled.content, "state"), "running");

    let stopped = tool
        .call(
            json!({"action": "stop", "process_id": process_id}),
            tool_context(),
            "stop-call".into(),
        )
        .await
        .unwrap();
    assert!(
        stopped.content.lines().any(|line| line == "stop requested"),
        "{}",
        stopped.content
    );
    eventually(&manager, &process_id).await;
}

#[tokio::test]
async fn process_tool_rejects_invalid_action_arguments() {
    let tool = Process::new(ProcessManager::new(ProcessLimits::default()));
    for args in [
        json!({"action": "start"}),
        json!({"action": "poll"}),
        json!({"action": "stop"}),
        json!({"action": "write", "process_id": "unused"}),
    ] {
        assert!(matches!(
            tool.call(args, tool_context(), "call".into()).await,
            Err(ToolError::InvalidArguments(_))
        ));
    }

    let error = tool
        .call(
            json!({"action": "poll", "process_id": "unused", "wait_seconds": 31}),
            tool_context(),
            "call".into(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "wait_seconds must be between 0 and 30");
}

#[tokio::test]
async fn managed_process_stdin_is_closed() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(STDIN_CLOSED_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
    let snapshot = manager
        .poll(&started.process_id, Some(0), Duration::ZERO)
        .await
        .unwrap();

    assert!(snapshot.chunks.iter().any(|chunk| chunk.text == "closed"));
}

#[tokio::test]
async fn captures_streams_and_incremental_cursors() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(MIXED_OUTPUT_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let done = eventually(&manager, &started.process_id).await;
    let all = manager
        .poll(&started.process_id, Some(0), Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(done.state, State::Exited);
    assert!(all
        .chunks
        .iter()
        .any(|c| c.stream == Stream::Stdout && c.text.contains("out")));
    assert!(all
        .chunks
        .iter()
        .any(|c| c.stream == Stream::Stderr && c.text.contains("err")));
    let empty = manager
        .poll(&started.process_id, Some(all.next_cursor), Duration::ZERO)
        .await
        .unwrap();
    assert!(empty.chunks.is_empty());
}

#[tokio::test]
async fn stale_cursor_and_byte_and_chunk_limits_are_explicit() {
    let manager = ProcessManager::new(ProcessLimits {
        max_bytes: 3,
        max_chunks: 1,
        ..ProcessLimits::default()
    });
    let started = manager
        .start(
            LIMITED_OUTPUT_COMMAND.into(),
            std::path::Path::new("."),
            None,
        )
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
    let snapshot = manager
        .poll(&started.process_id, Some(0), Duration::ZERO)
        .await
        .unwrap();
    assert!(snapshot.truncated || snapshot.chunks.iter().map(|c| c.text.len()).sum::<usize>() <= 3);
    assert!(snapshot.chunks.len() <= 1);
}

#[tokio::test]
async fn long_poll_observes_output_without_missed_wakeup() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(
            DELAYED_OUTPUT_COMMAND.into(),
            std::path::Path::new("."),
            None,
        )
        .await
        .unwrap();
    let polling = {
        let manager = manager.clone();
        let id = started.process_id.clone();
        tokio::spawn(async move {
            manager
                .poll(&id, Some(0), Duration::from_secs(5))
                .await
                .unwrap()
        })
    };
    let snapshot = polling.await.unwrap();
    assert!(
        snapshot.chunks.iter().any(|c| c.text.contains("wake")),
        "{snapshot:?}"
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn enforces_live_limit_and_shutdown_is_terminal() {
    let manager = ProcessManager::new(ProcessLimits {
        max_live: 1,
        ..ProcessLimits::default()
    });
    let first = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    assert_eq!(
        manager
            .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
            .await
            .unwrap_err(),
        "live process limit reached"
    );
    manager.shutdown().await;
    assert_eq!(
        manager
            .poll(&first.process_id, None, Duration::ZERO)
            .await
            .unwrap()
            .state,
        State::Terminated
    );
}

#[tokio::test]
async fn timeout_and_stop_reach_distinct_terminal_states() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let timeout = manager
        .start(
            LONG_RUNNING_COMMAND.into(),
            std::path::Path::new("."),
            Some(Duration::from_millis(20)),
        )
        .await
        .unwrap();
    assert_eq!(
        eventually(&manager, &timeout.process_id).await.state,
        State::TimedOut
    );
    let stopped = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    manager
        .stop(&stopped.process_id, Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        eventually(&manager, &stopped.process_id).await.state,
        State::Terminated
    );
}

#[tokio::test]
async fn drains_all_output_before_marking_terminal() {
    let manager = ProcessManager::new(ProcessLimits {
        max_bytes: 2_000_000,
        ..ProcessLimits::default()
    });
    let started = manager
        .start(LARGE_OUTPUT_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
    let snapshot = manager
        .poll(&started.process_id, Some(0), Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(
        snapshot
            .chunks
            .iter()
            .map(|chunk| chunk.text.len())
            .sum::<usize>(),
        1_000_000
    );
}

#[tokio::test]
async fn retained_record_limit_removes_oldest_completed_records() {
    let manager = ProcessManager::new(ProcessLimits {
        max_records: 2,
        ..ProcessLimits::default()
    });
    let mut ids = Vec::new();
    for command in [SUCCESS_COMMAND, SUCCESS_COMMAND, SUCCESS_COMMAND] {
        let started = manager
            .start(command.into(), std::path::Path::new("."), None)
            .await
            .unwrap();
        eventually(&manager, &started.process_id).await;
        ids.push(started.process_id);
    }
    let fourth = manager
        .start(SUCCESS_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    eventually(&manager, &fourth.process_id).await;
    assert!(manager.poll(&ids[0], None, Duration::ZERO).await.is_err());
}

#[tokio::test]
async fn concurrent_poll_and_stop_do_not_deadlock() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let id = started.process_id;
    let operations = (0..20)
        .map(|_| {
            let manager = manager.clone();
            let id = id.clone();
            tokio::spawn(async move {
                let _ = manager.poll(&id, None, Duration::from_millis(5)).await;
            })
        })
        .collect::<Vec<_>>();
    manager.stop(&id, Duration::ZERO).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        futures_util::future::join_all(operations),
    )
    .await
    .unwrap();
    eventually(&manager, &id).await;
}

#[cfg(unix)]
fn process_is_running(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == -1 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        // A container's PID 1 may not reap orphaned grandchildren. Zombies are
        // terminated even though kill(pid, 0) continues to find their PID.
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            return true;
        };
        !fields.starts_with("Z ")
    }
    #[cfg(not(target_os = "linux"))]
    true
}

#[cfg(unix)]
async fn descendant_case(action: &str) {
    let directory = tempfile::tempdir().unwrap();
    let pid_file = directory.path().join("pid");
    let command = format!("sleep 300 & echo $! > {}; wait", pid_file.display());
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(
            command,
            std::path::Path::new("."),
            (action == "timeout").then_some(Duration::from_millis(500)),
        )
        .await
        .unwrap();
    let pid = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|contents| contents.trim().parse::<i32>().ok())
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("descendant pid was not written");
    match action {
        "stop" => manager
            .stop(&started.process_id, Duration::ZERO)
            .await
            .unwrap(),
        "shutdown" => manager.shutdown().await,
        "drop" => drop(manager),
        "timeout" => {
            eventually(&manager, &started.process_id).await;
        }
        _ => unreachable!(),
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while process_is_running(pid) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("descendant {pid} survived {action}"));
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_stop_kills_descendants() {
    descendant_case("stop").await;
}
#[cfg(unix)]
#[tokio::test]
async fn timeout_kills_descendants() {
    descendant_case("timeout").await;
}
#[cfg(unix)]
#[tokio::test]
async fn async_shutdown_kills_descendants() {
    descendant_case("shutdown").await;
}
#[cfg(unix)]
#[tokio::test]
async fn drop_kills_descendants() {
    descendant_case("drop").await;
}

#[cfg(unix)]
#[test]
fn managed_process_http_server_fixture() {
    use std::io::{Read, Write};

    if std::env::var_os("RHO_PROCESS_SERVER_FIXTURE").is_none() {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    println!(
        "RHO_PROCESS_SERVER {} {}",
        std::process::id(),
        listener.local_addr().unwrap().port()
    );
    std::io::stdout().flush().unwrap();
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request);
        let body = "rho-coding-agent";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn local_server_e2e_start_poll_access_no_duplicate_and_stop() {
    use std::net::TcpListener;
    let manager = ProcessManager::new(ProcessLimits::default());
    let executable = std::env::current_exe()
        .unwrap()
        .to_string_lossy()
        .replace('\'', "'\\''");
    let command = format!(
        "RHO_PROCESS_SERVER_FIXTURE=1 '{executable}' --exact tools::process::tests::managed_process_http_server_fixture --nocapture"
    );
    let started = manager
        .start(command, std::path::Path::new("."), None)
        .await
        .unwrap();
    let (pid, port, output_cursor) = tokio::time::timeout(Duration::from_secs(15), async {
        let mut cursor = 0;
        let mut stdout = String::new();
        loop {
            let snapshot = manager
                .poll(&started.process_id, Some(cursor), Duration::from_secs(5))
                .await
                .unwrap();
            cursor = snapshot.next_cursor;
            stdout.extend(
                snapshot
                    .chunks
                    .iter()
                    .filter(|chunk| chunk.stream == Stream::Stdout)
                    .map(|chunk| chunk.text.as_str()),
            );
            let fields = stdout.split_whitespace().collect::<Vec<_>>();
            if let Some(marker) = fields
                .windows(3)
                .find(|fields| fields[0] == "RHO_PROCESS_SERVER")
            {
                let pid = marker[1].parse::<i32>().unwrap();
                let port = marker[2].parse::<u16>().unwrap();
                break (pid, port, cursor);
            }
            assert!(
                !terminal(snapshot.state),
                "server exited before reporting its address: {snapshot:?}"
            );
        }
    })
    .await
    .expect("server did not report its address");
    let url = format!("http://127.0.0.1:{port}/Cargo.toml");
    let body = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Ok(response) = reqwest::get(&url).await {
                break response.text().await.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("server did not accept connections");
    assert!(body.contains("rho-coding-agent"));
    let after_access = manager
        .poll(
            &started.process_id,
            Some(output_cursor),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    let duplicate = manager
        .poll(
            &started.process_id,
            Some(after_access.next_cursor),
            Duration::ZERO,
        )
        .await
        .unwrap();
    assert!(duplicate.chunks.is_empty());
    manager
        .stop(&started.process_id, Duration::ZERO)
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while process_is_running(pid) || TcpListener::bind(("127.0.0.1", port)).is_err() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("stopped server did not release its process or port");
}

#[tokio::test]
async fn concurrent_starts_atomically_enforce_live_limit() {
    let manager = ProcessManager::new(ProcessLimits {
        max_live: 1,
        ..ProcessLimits::default()
    });
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let starts = (0..2)
        .map(|_| {
            let manager = manager.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                manager
                    .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
                    .await
            })
        })
        .collect::<Vec<_>>();
    barrier.wait().await;
    let results = futures_util::future::join_all(starts).await;
    assert_eq!(
        results
            .iter()
            .filter(|result| result.as_ref().unwrap().is_ok())
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result
                .as_ref()
                .unwrap()
                .as_ref()
                .is_err_and(|error| error == "live process limit reached"))
            .count(),
        1
    );
    manager.shutdown().await;
}

#[tokio::test]
async fn invalid_utf8_retention_uses_raw_byte_cost() {
    let manager = ProcessManager::new(ProcessLimits {
        max_bytes: 2,
        max_chunks: 10,
        ..ProcessLimits::default()
    });
    let started = manager
        .start(
            "printf '\\377\\377\\377'".into(),
            std::path::Path::new("."),
            None,
        )
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
    let snapshot = manager
        .poll(&started.process_id, Some(0), Duration::ZERO)
        .await
        .unwrap();
    assert!(snapshot.chunks.is_empty());
    assert!(snapshot.truncated);
}

#[tokio::test]
async fn bounded_poll_advances_only_over_delivered_chunks() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(
            "printf first; sleep .05; printf second".into(),
            std::path::Path::new("."),
            None,
        )
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
    let all = manager
        .poll(&started.process_id, Some(0), Duration::ZERO)
        .await
        .unwrap();
    assert!(all.chunks.len() >= 2, "{all:?}");
    let budget = all
        .chunks
        .iter()
        .map(|chunk| {
            let mut one = all.clone();
            one.chunks = vec![chunk.clone()];
            one.next_cursor = chunk.cursor + 1;
            one.output_pending = true;
            super::output::format_snapshot(&one).len()
        })
        .max()
        .unwrap();
    let mut both = all.clone();
    both.output_pending = true;
    assert!(
        budget < super::output::format_snapshot(&both).len(),
        "one-chunk budget {budget} must not fit both chunks"
    );
    let one = manager
        .poll_bounded(&started.process_id, Some(0), Duration::ZERO, budget)
        .await
        .unwrap();
    assert_eq!(one.chunks.len(), 1);
    assert!(one.output_pending);
    assert!(one.next_cursor < one.available_cursor);
    let two = manager
        .poll_bounded(
            &started.process_id,
            Some(one.next_cursor),
            Duration::ZERO,
            budget,
        )
        .await
        .unwrap();
    assert_eq!(two.chunks.len(), 1);
    assert_ne!(one.chunks[0].text, two.chunks[0].text);
}

#[tokio::test]
async fn bounded_poll_skips_a_chunk_larger_than_the_budget() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(
            "printf 'a very large first chunk'; sleep .05; printf later".into(),
            std::path::Path::new("."),
            None,
        )
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
    let skipped = manager
        .poll_bounded(&started.process_id, Some(0), Duration::ZERO, 2)
        .await
        .unwrap();
    assert!(skipped.chunks.is_empty());
    assert!(skipped.next_cursor > 0);
    let later = manager
        .poll_bounded(
            &started.process_id,
            Some(skipped.next_cursor),
            Duration::ZERO,
            usize::MAX,
        )
        .await
        .unwrap();
    assert!(later
        .chunks
        .iter()
        .any(|chunk| chunk.text.contains("later")));
}

#[tokio::test]
async fn aborted_stop_caller_does_not_cancel_request_or_cleanup() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let stop = {
        let manager = manager.clone();
        let id = started.process_id.clone();
        tokio::spawn(async move { manager.stop(&id, Duration::ZERO).await })
    };
    tokio::task::yield_now().await;
    stop.abort();
    assert_eq!(
        eventually(&manager, &started.process_id).await.state,
        State::Terminated
    );
}

// Covers: the host rail must see live jobs.
// Owner: process manager
#[tokio::test]
async fn live_summaries_lists_running() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();

    let summaries = manager.live_summaries();
    assert_eq!(summaries.len(), 1);
    pretty_assertions::assert_eq!(
        (
            summaries[0].process_id.as_str(),
            summaries[0].command.as_str(),
            terminal(summaries[0].state),
            summaries[0].quiet_seconds,
            summaries[0].exit_code
        ),
        (
            started.process_id.as_str(),
            LONG_RUNNING_COMMAND,
            false,
            None,
            None
        )
    );

    manager
        .stop(&started.process_id, Duration::ZERO)
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
}

// Covers: overflow rows must keep the oldest live process, not the newest.
// Owner: process manager
#[tokio::test]
async fn live_summaries_orders_oldest_first() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let first = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let second = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();

    let ids = manager
        .live_summaries()
        .into_iter()
        .map(|summary| summary.process_id)
        .collect::<Vec<_>>();
    pretty_assertions::assert_eq!(
        ids,
        vec![first.process_id.clone(), second.process_id.clone()]
    );

    manager
        .stop(&first.process_id, Duration::ZERO)
        .await
        .unwrap();
    manager
        .stop(&second.process_id, Duration::ZERO)
        .await
        .unwrap();
    eventually(&manager, &first.process_id).await;
    eventually(&manager, &second.process_id).await;
}

// Covers: a just-finished process must linger on the rail with a frozen elapsed
// duration and its exit code, not disappear or keep ticking.
// Owner: process manager
#[tokio::test]
async fn live_summaries_lingers_terminal_rows_with_frozen_elapsed_and_exit_code() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(SUCCESS_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;

    let summaries = manager.live_summaries();
    assert_eq!(summaries.len(), 1);
    pretty_assertions::assert_eq!(
        (
            summaries[0].process_id.as_str(),
            summaries[0].command.as_str(),
            summaries[0].state,
            summaries[0].quiet_seconds,
            summaries[0].exit_code
        ),
        (
            started.process_id.as_str(),
            SUCCESS_COMMAND,
            State::Exited,
            None,
            Some(0)
        )
    );
    // `true` finishes immediately; frozen elapsed is completed-started, not wall
    // time since start, so it stays at a truncated 0s (or 1s if the spawn
    // straddled a second).
    assert!(
        summaries[0].elapsed_seconds <= 1,
        "elapsed_seconds={}",
        summaries[0].elapsed_seconds
    );
}

// Covers: the rail reports seconds-since-output for live jobs, and None when
// nothing has been written yet.
// Owner: process manager
#[tokio::test]
async fn live_summaries_reports_quiet_seconds_only_after_output() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let silent = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let noisy = manager
        .start(
            OUTPUT_THEN_SLEEP_COMMAND.into(),
            std::path::Path::new("."),
            None,
        )
        .await
        .unwrap();

    loop {
        let snapshot = manager
            .poll(&noisy.process_id, Some(0), Duration::from_secs(2))
            .await
            .unwrap();
        if snapshot
            .chunks
            .iter()
            .any(|chunk| chunk.text.contains("hello"))
        {
            break;
        }
        assert!(
            !terminal(snapshot.state),
            "noisy process exited before producing output"
        );
    }

    let summaries = manager.live_summaries();
    pretty_assertions::assert_eq!(
        summaries
            .iter()
            .map(|summary| summary.process_id.as_str())
            .collect::<Vec<_>>(),
        vec![silent.process_id.as_str(), noisy.process_id.as_str()]
    );
    pretty_assertions::assert_eq!(summaries[0].quiet_seconds, None);
    pretty_assertions::assert_eq!(summaries[0].exit_code, None);
    match summaries[1].quiet_seconds {
        Some(seconds) => assert!(seconds <= 2, "quiet_seconds={seconds}"),
        None => panic!("expected quiet_seconds after output"),
    }
    pretty_assertions::assert_eq!(summaries[1].exit_code, None);

    manager
        .stop(&silent.process_id, Duration::ZERO)
        .await
        .unwrap();
    manager
        .stop(&noisy.process_id, Duration::ZERO)
        .await
        .unwrap();
    eventually(&manager, &silent.process_id).await;
    eventually(&manager, &noisy.process_id).await;
}

async fn wait_for_exit_notification(manager: &ProcessManager) -> Vec<ProcessNotification> {
    loop {
        let notified = manager.notified_owned();
        let notifications = manager.take_notifications();
        if !notifications.is_empty() {
            return notifications;
        }
        tokio::time::timeout(Duration::from_secs(2), notified)
            .await
            .expect("process should become terminal");
    }
}

// Covers: a finished process is delivered once at the turn boundary unless poll already observed it
// Owner: process manager
#[tokio::test]
async fn finished_process_is_delivered_once_until_restored() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(SUCCESS_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let notifications = wait_for_exit_notification(&manager).await;
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].process_id, started.process_id);
    assert_eq!(notifications[0].state, State::Exited);
    assert!(manager.take_notifications().is_empty());

    manager.restore_notifications(&notifications);
    let again = manager.take_notifications();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].process_id, started.process_id);
}

// Covers: a terminal poll counts as delivery so idle wake does not repeat it
// Owner: process manager
#[tokio::test]
async fn terminal_poll_observes_and_suppresses_notification() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(SUCCESS_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let snapshot = eventually(&manager, &started.process_id).await;
    assert!(terminal(snapshot.state));
    assert!(
        manager.take_notifications().is_empty(),
        "poll of a finished process must consume the automatic delivery"
    );
}

// Covers: a live background process must not pin the idle loop
// Owner: process manager
#[tokio::test]
async fn running_process_is_not_a_pending_notification() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(LONG_RUNNING_COMMAND.into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    assert!(
        !manager.has_pending_notification(),
        "running jobs wait on Notify, not the 500ms idle tick"
    );
    manager
        .stop(&started.process_id, Duration::ZERO)
        .await
        .unwrap();
    let _ = wait_for_exit_notification(&manager).await;
    assert!(manager.take_notifications().is_empty());
}

// Covers: a shell leader that exits before its descendants must not leave the
// record running forever or finalize with an unstoppable orphan tree
// Owner: process supervisor
#[cfg(unix)]
#[tokio::test]
async fn leader_exit_terminates_surviving_descendants() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("descendant.pid");
    // The background sleep inherits stdout, so without cleanup the pipe drain
    // would never reach EOF and the record would stay running.
    let command = format!("sleep 300 & echo $! > {}; exit 0", pid_file.display());
    let started = manager
        .start(command, std::path::Path::new("."), None)
        .await
        .unwrap();

    // Bounded so a regression fails fast: without tree cleanup the record
    // stays running until the descendant's own 300s sleep expires.
    let snapshot = tokio::time::timeout(
        Duration::from_secs(15),
        eventually(&manager, &started.process_id),
    )
    .await
    .expect("record must reach a terminal state once the leader exits");
    assert_eq!(snapshot.state, State::Exited);
    assert_eq!(snapshot.exit_code, Some(0));

    let pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    // A reparented descendant can linger as an unreaped zombie whose parent is
    // a non-reaping init, so also accept a Linux Z state as gone.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut alive = unsafe { libc::kill(pid, 0) } == 0;
        #[cfg(target_os = "linux")]
        if alive {
            alive &= std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| {
                    // Field 3 of /proc/<pid>/stat; safe here because the comm
                    // field of `sleep` contains no spaces.
                    stat.split_whitespace().nth(2).map(|state| state != "Z")
                })
                .unwrap_or(false);
        }
        if !alive {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "descendant {pid} survived leader exit"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Shutdown must not hang on the finalized record.
    manager.shutdown().await;
}
