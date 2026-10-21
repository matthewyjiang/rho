use super::*;

/// Covers: `terminate` stops a long-running child and `wait` returns without
/// hanging on it.
/// Owner: `OwnedChild::terminate` and the process-group kill it drives.
#[tokio::test]
async fn owned_child_terminate_stops_running_process() {
    #[cfg(unix)]
    let cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", "sleep 30"]);
        c
    };
    #[cfg(windows)]
    let cmd = {
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
        c
    };

    let mut child = OwnedChild::spawn(cmd).expect("spawn child");
    // The child sleeps far longer than the budget, so a terminate that fails
    // to kill the group would run out the sleep and pass. Fail loud instead.
    // Budget: 25x TERMINATION_GRACE_PERIOD, well under the 30 s sleep.
    let budget = TERMINATION_GRACE_PERIOD * 25;
    let status = tokio::time::timeout(budget, async {
        child.terminate().await;
        child.wait().await
    })
    .await
    .unwrap_or_else(|_| panic!("terminate did not stop the child within {budget:?}"))
    .expect("wait on terminated child");
    assert!(!status.success());
}
