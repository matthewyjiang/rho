use super::*;
use pretty_assertions::assert_eq;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread;
use std::time::Duration;

#[test]
fn converts_usd_amounts_to_micros() {
    assert_eq!(usd_to_micros(0.0388), 38_800);
    assert_eq!(usd_to_micros(1.5), 1_500_000);
    assert_eq!(usd_to_micros(0.0), 0);
    assert_eq!(usd_to_micros(-1.0), 0);
    assert_eq!(usd_to_micros(f64::NAN), 0);
}

#[test]
fn write_status_repeatedly_replaces_existing_result_json() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    let mut status = RunStatus {
        state: RunState::Starting,
        agent_id: Some("alpha".into()),
        last_activity: Some("starting".into()),
        ..RunStatus::default()
    };
    write_status(&path, &status).unwrap();
    assert_eq!(read_status(&path).unwrap().state, RunState::Starting);

    status.state = RunState::Running;
    status.last_activity = Some("running".into());
    write_status(&path, &status).unwrap();
    assert_eq!(read_status(&path).unwrap().state, RunState::Running);

    status.state = RunState::Ok;
    status.result = Some("done".into());
    write_status(&path, &status).unwrap();
    let loaded = read_status(&path).unwrap();
    assert_eq!(loaded.state, RunState::Ok);
    assert_eq!(loaded.result.as_deref(), Some("done"));
}

// Covers: a later status write without a title must keep the generated title.
// Owner: run status persistence
#[test]
fn write_status_preserves_an_existing_title() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    let titled = RunStatus {
        state: RunState::Running,
        agent_id: Some("worker".into()),
        title: Some("Review the auth path".into()),
        last_activity: Some("tool: read".into()),
        ..RunStatus::default()
    };
    write_status(&path, &titled).unwrap();

    let update = RunStatus {
        state: RunState::Running,
        agent_id: Some("worker".into()),
        last_activity: Some("responding".into()),
        ..RunStatus::default()
    };
    write_status(&path, &update).unwrap();
    let loaded = read_status(&path).unwrap();
    assert_eq!(loaded.title.as_deref(), Some("Review the auth path"));
    assert_eq!(loaded.last_activity.as_deref(), Some("responding"));
}

#[test]
fn terminal_status_is_not_overwritten_by_nonterminal() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    let terminal = RunStatus {
        state: RunState::Error,
        agent_id: Some("alpha".into()),
        error: Some("fallback error".into()),
        ..RunStatus::default()
    };
    write_status(&path, &terminal).unwrap();

    let running = RunStatus {
        state: RunState::Running,
        agent_id: Some("alpha".into()),
        last_activity: Some("stale running".into()),
        ..RunStatus::default()
    };
    write_status(&path, &running).unwrap();
    let loaded = read_status(&path).unwrap();
    assert_eq!(loaded.state, RunState::Error);
    assert_eq!(loaded.agent_id.as_deref(), Some("alpha"));
    assert_eq!(loaded.error.as_deref(), Some("fallback error"));
    assert!(loaded.finished_at.is_some());

    // Terminal-to-terminal remains allowed (for example Stopped after cancel).
    let stopped = RunStatus {
        state: RunState::Stopped,
        agent_id: Some("alpha".into()),
        last_activity: Some("cancelled".into()),
        ..RunStatus::default()
    };
    write_status(&path, &stopped).unwrap();
    assert_eq!(read_status(&path).unwrap().state, RunState::Stopped);

    // Normal starting -> running -> terminal still works on a fresh file.
    let path2 = directory.path().join("fresh.json");
    write_status(
        &path2,
        &RunStatus {
            state: RunState::Starting,
            ..RunStatus::default()
        },
    )
    .unwrap();
    write_status(
        &path2,
        &RunStatus {
            state: RunState::Running,
            ..RunStatus::default()
        },
    )
    .unwrap();
    write_status(
        &path2,
        &RunStatus {
            state: RunState::Ok,
            result: Some("done".into()),
            ..RunStatus::default()
        },
    )
    .unwrap();
    let loaded = read_status(&path2).unwrap();
    assert_eq!(loaded.state, RunState::Ok);
    assert_eq!(loaded.result.as_deref(), Some("done"));
}

#[test]
fn initialize_status_replaces_prior_terminal_for_new_run() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);

    write_status(
        &path,
        &RunStatus {
            state: RunState::Ok,
            agent_id: Some("old".into()),
            result: Some("previous run".into()),
            ..RunStatus::default()
        },
    )
    .unwrap();
    assert_eq!(read_status(&path).unwrap().state, RunState::Ok);

    // Same-run monotonic write must not demote the prior terminal file.
    write_status(
        &path,
        &RunStatus {
            state: RunState::Starting,
            agent_id: Some("new".into()),
            ..RunStatus::default()
        },
    )
    .unwrap();
    assert_eq!(read_status(&path).unwrap().state, RunState::Ok);

    // New-run initialization deliberately replaces the prior terminal snapshot.
    initialize_status(
        &path,
        &RunStatus {
            state: RunState::Starting,
            agent_id: Some("new".into()),
            last_activity: Some("starting".into()),
            ..RunStatus::default()
        },
    )
    .unwrap();
    assert_eq!(read_status(&path).unwrap().state, RunState::Starting);
    assert_eq!(read_status(&path).unwrap().agent_id.as_deref(), Some("new"));

    write_status(
        &path,
        &RunStatus {
            state: RunState::Running,
            agent_id: Some("new".into()),
            last_activity: Some("running".into()),
            ..RunStatus::default()
        },
    )
    .unwrap();
    assert_eq!(read_status(&path).unwrap().state, RunState::Running);

    write_status(
        &path,
        &RunStatus {
            state: RunState::Error,
            agent_id: Some("new".into()),
            error: Some("new run failed".into()),
            ..RunStatus::default()
        },
    )
    .unwrap();
    let loaded = read_status(&path).unwrap();
    assert_eq!(loaded.state, RunState::Error);
    assert_eq!(loaded.error.as_deref(), Some("new run failed"));
    assert_eq!(loaded.agent_id.as_deref(), Some("new"));
}

/// Proves the read-check-replace critical section is owned: a Running writer
/// that has already read cannot lose the race to a terminal fallback. The
/// fallback blocks on shared ownership until the section finishes, then writes
/// Error last so the final on-disk state is terminal.
#[test]
fn concurrent_terminal_wins_over_stale_running_after_read() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    write_status(
        &path,
        &RunStatus {
            state: RunState::Starting,
            agent_id: Some("alpha".into()),
            ..RunStatus::default()
        },
    )
    .unwrap();

    struct ClearHook;
    impl Drop for ClearHook {
        fn drop(&mut self) {
            status_write_hooks::clear();
        }
    }
    let _clear_hook = ClearHook;

    let reached_after_read = Arc::new((Mutex::new(false), Condvar::new()));
    let allow_replace = Arc::new((Mutex::new(false), Condvar::new()));
    let running_started = Arc::new(AtomicBool::new(false));

    {
        let target_path = path.clone();
        let reached_after_read = Arc::clone(&reached_after_read);
        let allow_replace = Arc::clone(&allow_replace);
        let running_started = Arc::clone(&running_started);
        status_write_hooks::set_after_read(move |hook_path, status| {
            if hook_path != target_path.as_path()
                || status.state != RunState::Running
                || running_started.swap(true, Ordering::SeqCst)
            {
                return;
            }
            {
                let (lock, cv) = &*reached_after_read;
                let mut ready = lock.lock().unwrap_or_else(|p| p.into_inner());
                *ready = true;
                cv.notify_all();
            }
            let (lock, cv) = &*allow_replace;
            let mut go = lock.lock().unwrap_or_else(|p| p.into_inner());
            while !*go {
                go = cv.wait(go).unwrap_or_else(|p| p.into_inner());
            }
        });
    }

    let path_running = path.clone();
    let running_thread = thread::spawn(move || {
        write_status(
            &path_running,
            &RunStatus {
                state: RunState::Running,
                agent_id: Some("alpha".into()),
                last_activity: Some("stale running".into()),
                ..RunStatus::default()
            },
        )
        .unwrap();
    });

    {
        let (lock, cv) = &*reached_after_read;
        let ready = lock.lock().unwrap_or_else(|p| p.into_inner());
        let (ready, timeout) = cv
            .wait_timeout_while(ready, Duration::from_secs(2), |ready| !*ready)
            .unwrap_or_else(|p| p.into_inner());
        assert!(
            *ready && !timeout.timed_out(),
            "running writer never reached post-read hook"
        );
    }

    // Fallback attempts terminal write while Running is paused after its read.
    // Shared ownership blocks here until Running finishes its section.
    let path_terminal = path.clone();
    let terminal_thread = thread::spawn(move || {
        write_status(
            &path_terminal,
            &RunStatus {
                state: RunState::Error,
                agent_id: Some("alpha".into()),
                error: Some("fallback error".into()),
                ..RunStatus::default()
            },
        )
        .unwrap();
    });

    // Give the terminal writer a chance to block on the status lock.
    thread::sleep(Duration::from_millis(30));

    {
        let (lock, cv) = &*allow_replace;
        let mut go = lock.lock().unwrap_or_else(|p| p.into_inner());
        *go = true;
        cv.notify_all();
    }

    running_thread.join().unwrap();
    terminal_thread.join().unwrap();

    let loaded = read_status(&path).unwrap();
    assert_eq!(loaded.state, RunState::Error);
    assert_eq!(loaded.error.as_deref(), Some("fallback error"));
}

#[test]
fn normalize_id_lowercases_hex_and_rejects_invalid() {
    assert_eq!(normalize_id("26BC7E").unwrap(), "26bc7e");
    assert_eq!(normalize_id("abcdef").unwrap(), "abcdef");
    assert!(normalize_id("26bc7").is_err());
    assert!(normalize_id("26bc7g").is_err());
    assert!(normalize_id("zzzzzz").is_err());
}

#[test]
fn write_status_omits_unknown_token_counters() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    write_status(
        &path,
        &RunStatus {
            state: RunState::Stopped,
            last_activity: Some("cancelled".into()),
            ..RunStatus::default()
        },
    )
    .unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(!raw.contains("input_tokens"), "{raw}");
    assert!(!raw.contains("output_tokens"), "{raw}");
    let loaded = read_status(&path).unwrap();
    assert_eq!(loaded.input_tokens, None);
    assert_eq!(loaded.output_tokens, None);
}

#[test]
fn old_status_files_without_parent_session_id_still_parse() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    std::fs::write(
        &path,
        r#"{
  "state": "ok",
  "agent_id": "worker",
  "turns": 1
}"#,
    )
    .unwrap();

    let status = read_status(&path).unwrap();
    assert_eq!(status.state, RunState::Ok);
    assert_eq!(status.parent_session_id, None);
    assert_eq!(status.runtime, None);
    assert_eq!(status.started_at, None);
    assert_eq!(status.finished_at, None);
}

#[test]
fn parent_session_id_round_trips_in_status_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    let status = RunStatus {
        state: RunState::Starting,
        parent_session_id: Some("parent-session-id".into()),
        agent_id: Some("worker".into()),
        ..RunStatus::default()
    };
    initialize_status(&path, &status).unwrap();
    assert_eq!(
        read_status(&path).unwrap().parent_session_id.as_deref(),
        Some("parent-session-id")
    );
}

#[test]
fn runtime_and_timing_fields_round_trip_in_status_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    let status = RunStatus {
        state: RunState::Ok,
        agent_id: Some("worker".into()),
        runtime: Some(crate::agent::AgentRuntime::ClaudeCli),
        started_at: Some(1_700_000_000),
        finished_at: Some(1_700_000_042),
        ..RunStatus::default()
    };
    initialize_status(&path, &status).unwrap();
    let loaded = read_status(&path).unwrap();
    assert_eq!(loaded.runtime, Some(crate::agent::AgentRuntime::ClaudeCli));
    assert_eq!(loaded.started_at, Some(1_700_000_000));
    assert_eq!(loaded.finished_at, Some(1_700_000_042));
    assert_eq!(
        loaded
            .elapsed_duration(9_999)
            .map(|elapsed| elapsed.as_secs()),
        Some(42)
    );
}

#[test]
fn write_status_stamps_finished_at_for_terminal_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    let status = RunStatus {
        state: RunState::Ok,
        agent_id: Some("worker".into()),
        started_at: Some(1_700_000_000),
        ..RunStatus::default()
    };
    write_status(&path, &status).unwrap();
    let loaded = read_status(&path).unwrap();
    assert!(loaded.finished_at.is_some());
    assert!(loaded.finished_at.unwrap() >= loaded.started_at.unwrap());
}

// Covers: a late title stamp must not rewrite result, activity, or finish time.
// Owner: subagent status persistence
#[test]
fn apply_generated_title_preserves_other_fields() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    write_status(
        &path,
        &RunStatus {
            state: RunState::Ok,
            agent_id: Some("worker".into()),
            result: Some("done".into()),
            last_activity: Some("finished".into()),
            finished_at: Some(1_700_000_042),
            ..RunStatus::default()
        },
    )
    .unwrap();

    apply_generated_title(&path, "Review the auth path").unwrap();

    let loaded = read_status(&path).unwrap();
    assert_eq!(loaded.title.as_deref(), Some("Review the auth path"));
    assert_eq!(loaded.state, RunState::Ok);
    assert_eq!(loaded.result.as_deref(), Some("done"));
    assert_eq!(loaded.last_activity.as_deref(), Some("finished"));
    assert_eq!(loaded.finished_at, Some(1_700_000_042));
}

#[test]
fn write_status_preserves_existing_terminal_finished_at() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(RESULT_FILE_NAME);
    let original = RunStatus {
        state: RunState::Error,
        agent_id: Some("worker".into()),
        started_at: Some(1_700_000_000),
        finished_at: Some(1_700_000_042),
        error: Some("first failure".into()),
        ..RunStatus::default()
    };
    write_status(&path, &original).unwrap();
    assert_eq!(read_status(&path).unwrap().finished_at, Some(1_700_000_042));

    // Later terminal snapshot omits finished_at; keep the first durable finish.
    let upgraded = RunStatus {
        state: RunState::Stopped,
        agent_id: Some("worker".into()),
        started_at: Some(1_700_000_000),
        last_activity: Some("cancelled after error".into()),
        error: Some("first failure".into()),
        ..RunStatus::default()
    };
    write_status(&path, &upgraded).unwrap();
    let loaded = read_status(&path).unwrap();
    assert_eq!(loaded.state, RunState::Stopped);
    assert_eq!(loaded.finished_at, Some(1_700_000_042));
}

#[test]
fn format_elapsed_secs_covers_second_minute_and_hour_buckets() {
    assert_eq!(format_elapsed_secs(12), "12s");
    assert_eq!(format_elapsed_secs(65), "1m 05s");
    assert_eq!(format_elapsed_secs(3_725), "1h 02m");
}
