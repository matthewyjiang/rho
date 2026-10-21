use super::WindowsJob;
use tokio::{io::AsyncReadExt, process::Command};

const FIXTURE_TEST: &str = "process_supervision::tests::job_descendant_fixture";
const FIXTURE_ROLE: &str = "RHO_JOB_FIXTURE_ROLE";
const FIXTURE_PORT: &str = "RHO_JOB_FIXTURE_PORT";

/// Re-runs this test binary as `job_descendant_fixture` in `role`. The test
/// binary starts in milliseconds; two cold PowerShell starts took 26 s on
/// Windows CI and sometimes outlived the 60 s connect deadline.
fn fixture_command(role: &str, port: u16) -> std::process::Command {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", FIXTURE_TEST, "--nocapture"])
        .env(FIXTURE_ROLE, role)
        .env(FIXTURE_PORT, port.to_string());
    command
}

// Re-exec target for `dropping_job_terminates_descendants`, and a no-op when
// run as a normal test. The "parent" role spawns the "descendant" role and
// waits for it; the descendant connects to the test's listener and blocks on
// a read until something kills it.
#[test]
fn job_descendant_fixture() {
    let Some(role) = std::env::var_os(FIXTURE_ROLE) else {
        return;
    };
    let port: u16 = std::env::var(FIXTURE_PORT).unwrap().parse().unwrap();
    if role == "parent" {
        let _ = fixture_command("descendant", port).status();
        return;
    }
    let mut connection = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let _ = std::io::Read::read(&mut connection, &mut [0]);
}

// Covers: dropping a Windows job owner must terminate descendants, not just
// the immediate child. Owner: Windows process layer.
#[tokio::test]
async fn dropping_job_terminates_descendants() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut command = Command::from(fixture_command("parent", port));
    command.kill_on_drop(true);
    WindowsJob::prepare(&mut command);
    let mut child = command.spawn().unwrap();
    let job = WindowsJob::attach(&child).unwrap();
    // Match the inline shell's existing 60-second execution deadline. This only
    // bounds failures; accepting the connection synchronizes descendant startup.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let (mut connection, _) = tokio::time::timeout_at(deadline, listener.accept())
        .await
        .expect("descendant did not connect")
        .unwrap();
    drop(job);
    let mut byte = [0];
    let closed = tokio::time::timeout_at(deadline, connection.read(&mut byte)).await;
    drop(connection);
    let closed = closed.expect("descendant survived job closure");
    // Forced process termination may close the socket with FIN or RST.
    assert!(
        matches!(&closed, Ok(0))
            || matches!(&closed, Err(error) if matches!(error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted)),
        "descendant connection remains open: {closed:?}"
    );
    let status = tokio::time::timeout_at(deadline, child.wait())
        .await
        .expect("parent survived job closure")
        .unwrap();
    assert!(!status.success());
}
