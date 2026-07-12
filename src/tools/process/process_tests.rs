use super::types::{terminal, Stream};
use super::*;
use crate::tool::{Tool, ToolContext, ToolError};
use serde_json::json;
use std::{path::PathBuf, time::Duration};

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

#[test]
fn process_tool_has_one_compact_action_schema() {
    let tool = Process::new(ProcessManager::new(ProcessLimits::default()));
    let spec = tool.spec();

    assert_eq!(spec.name, "process");
    assert_eq!(
        spec.input_schema,
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["start", "poll", "stop"]},
                "command": {"type": "string"},
                "timeout_seconds": {"type": "integer", "minimum": 1},
                "process_id": {"type": "string"},
                "cursor": {"type": "integer", "minimum": 0},
                "wait_seconds": {"type": "integer", "minimum": 0, "maximum": 30}
            },
            "required": ["action"]
        })
    );
}

#[tokio::test]
async fn process_tool_dispatches_start_poll_and_stop() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let tool = Process::new(manager.clone());
    let started = tool
        .call(
            json!({"action": "start", "command": "sleep 300"}),
            tool_context(),
            "start-call".into(),
        )
        .await
        .unwrap();
    let started: serde_json::Value = serde_json::from_str(&started.content).unwrap();
    let process_id = started["process_id"].as_str().unwrap().to_owned();

    let polled = tool
        .call(
            json!({"action": "poll", "process_id": process_id}),
            tool_context(),
            "poll-call".into(),
        )
        .await
        .unwrap();
    let polled: serde_json::Value = serde_json::from_str(&polled.content).unwrap();
    assert_eq!(polled["state"], "running");

    let stopped = tool
        .call(
            json!({"action": "stop", "process_id": process_id}),
            tool_context(),
            "stop-call".into(),
        )
        .await
        .unwrap();
    let stopped: serde_json::Value = serde_json::from_str(&stopped.content).unwrap();
    assert_eq!(stopped["stop_requested"], true);
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
        .start(
            "read input || printf closed".into(),
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

    assert!(snapshot.chunks.iter().any(|chunk| chunk.text == "closed"));
}

#[tokio::test]
async fn captures_streams_and_incremental_cursors() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start(
            "printf out; printf err >&2".into(),
            std::path::Path::new("."),
            None,
        )
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
            "printf abc; printf def".into(),
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
            "sleep 0.05; printf wake".into(),
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
        .start("sleep 300".into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    assert_eq!(
        manager
            .start("sleep 300".into(), std::path::Path::new("."), None)
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
            "sleep 300".into(),
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
        .start("sleep 300".into(), std::path::Path::new("."), None)
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
        .start(
            "head -c 1000000 /dev/zero | tr '\\0' x".into(),
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
    for command in ["true", "true", "true"] {
        let started = manager
            .start(command.into(), std::path::Path::new("."), None)
            .await
            .unwrap();
        eventually(&manager, &started.process_id).await;
        ids.push(started.process_id);
    }
    let fourth = manager
        .start("true".into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    eventually(&manager, &fourth.process_id).await;
    assert!(manager.poll(&ids[0], None, Duration::ZERO).await.is_err());
}

#[tokio::test]
async fn concurrent_poll_and_stop_do_not_deadlock() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start("sleep 300".into(), std::path::Path::new("."), None)
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
    tokio::time::timeout(Duration::from_secs(5), async {
        while !pid_file.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("descendant pid file was not created");
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
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
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !process_is_running(pid),
        "descendant {pid} survived {action}"
    );
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
#[tokio::test]
async fn local_server_e2e_start_poll_access_no_duplicate_and_stop() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let manager = ProcessManager::new(ProcessLimits::default());
    let command = format!("python3 -m http.server {port} --bind 127.0.0.1 & echo $!; wait");
    let started = manager
        .start(command, std::path::Path::new("."), None)
        .await
        .unwrap();
    let first = manager
        .poll(&started.process_id, Some(0), Duration::from_secs(5))
        .await
        .unwrap();
    let pid: i32 = first
        .chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect::<String>()
        .trim()
        .parse()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}/Cargo.toml");
    let body = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(response) = reqwest::get(&url).await {
                break response.text().await.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("server did not accept connections");
    assert!(body.contains("rho-coding-agent"));
    let after_access = manager
        .poll(
            &started.process_id,
            Some(first.next_cursor),
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
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(TcpListener::bind(("127.0.0.1", port)).is_ok());
    assert!(!process_is_running(pid));
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
                    .start("sleep 300".into(), std::path::Path::new("."), None)
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
    let one = manager
        .poll_bounded(&started.process_id, Some(0), Duration::ZERO, 70)
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
            70,
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
        .start("sleep 300".into(), std::path::Path::new("."), None)
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
