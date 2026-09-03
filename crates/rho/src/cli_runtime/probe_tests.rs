use std::time::Duration;

use tokio::process::Command;

use super::*;

/// Covers: a hanging probe is killed at the injected timeout rather than
/// waiting out the child or the production budget.
/// Owner: bounded CLI probe machinery.
#[cfg(unix)]
#[tokio::test]
async fn bounded_probe_times_out_and_kills_hanging_child() {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg("sleep 30");
    let injected = Duration::from_millis(200);
    let started = std::time::Instant::now();
    let error = run_bounded_command_with_timeout(command, "/bin/sh".into(), injected)
        .await
        .unwrap_err();
    match error {
        ProbeError::Timeout { timeout, .. } => assert_eq!(timeout, injected),
        other => panic!("expected Timeout, got {other:?}"),
    }
    // Must not wait out a multi-second production budget or the child sleep.
    assert!(started.elapsed() < Duration::from_secs(2));
}
