use super::WindowsJob;
use tokio::{io::AsyncReadExt, process::Command};

// Covers: dropping a Windows job owner must terminate descendants, not just
// the immediate child. Owner: Windows process layer.
#[tokio::test]
async fn dropping_job_terminates_descendants() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let descendant = dir.path().join("descendant.ps1");
    std::fs::write(
        &descendant,
        format!(
            "$client = [System.Net.Sockets.TcpClient]::new('127.0.0.1', {port}); \
             $null = $client.GetStream().ReadByte()"
        ),
    )
    .unwrap();
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$p = Start-Process powershell.exe -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', ('\"' + $env:RHO_DESCENDANT_SCRIPT + '\"')) -PassThru; $p.WaitForExit()",
        ])
        .env("RHO_DESCENDANT_SCRIPT", &descendant)
        .kill_on_drop(true);
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
