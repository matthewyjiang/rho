use std::process::Stdio;

use pretty_assertions::assert_eq;

use super::*;

/// Covers: a spawned child exposes its piped stdin/stdout and `wait` reports a
/// clean exit once the leader is reaped.
/// Owner: `OwnedChild` spawn and wait.
#[tokio::test]
async fn owned_child_spawns_and_exits_cleanly() {
    #[cfg(unix)]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", "cat"]);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd.exe");
        c.args(["/c", "more"]);
        c
    };

    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = OwnedChild::spawn(cmd).expect("spawn child");
    let mut stdin = child.stdin().expect("stdin captured");
    let mut stdout = child.stdout().expect("stdout captured");

    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(b"test_output\n").await;
    });

    let mut buf = Vec::new();
    use tokio::io::AsyncReadExt;
    stdout.read_to_end(&mut buf).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&buf).trim(), "test_output");

    let status = child.wait().await.expect("wait on child");
    assert!(status.success());
}

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
    let mut cmd = {
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"]);
        c
    };

    let mut child = OwnedChild::spawn(cmd).expect("spawn child");
    child.terminate().await;
    let status = child.wait().await.expect("wait on terminated child");
    assert!(!status.success());
}
