use super::types::{terminal, Stream};
use super::*;
use std::time::Duration;

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
async fn stdin_close_drives_exit_and_subsequent_write_is_typed_error() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start("cat".into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    manager
        .write(&started.process_id, "hello\n", true)
        .await
        .unwrap();
    eventually(&manager, &started.process_id).await;
    assert_eq!(
        manager
            .write(&started.process_id, "again", false)
            .await
            .unwrap_err(),
        "process has exited"
    );
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
        .start("cat".into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    assert_eq!(
        manager
            .start("cat".into(), std::path::Path::new("."), None)
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
            "cat".into(),
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
        .start("cat".into(), std::path::Path::new("."), None)
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
    let listed = manager.list();
    assert_eq!(listed.len(), 2);
    assert!(manager.poll(&ids[0], None, Duration::ZERO).await.is_err());
}

#[tokio::test]
async fn concurrent_list_poll_and_stop_do_not_deadlock() {
    let manager = ProcessManager::new(ProcessLimits::default());
    let started = manager
        .start("cat".into(), std::path::Path::new("."), None)
        .await
        .unwrap();
    let id = started.process_id;
    let operations = (0..20)
        .map(|_| {
            let manager = manager.clone();
            let id = id.clone();
            tokio::spawn(async move {
                let _ = manager.list();
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
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
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
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}
