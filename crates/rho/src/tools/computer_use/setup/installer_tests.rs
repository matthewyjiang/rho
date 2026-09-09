#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt;

use super::*;

// Covers: a failed download must never execute even a syntactically valid prefix.
// Owner: installer process boundary. No network or real installer is used.
#[tokio::test]
async fn incomplete_download_is_not_executed() {
    let home = tempfile::tempdir().unwrap();
    let curl = home.path().join("curl");
    fs::write(
        &curl,
        "#!/bin/sh\nprintf '%s\\n' 'echo unsafe > \"$HOME/executed\"'\nexit 22\n",
    )
    .unwrap();
    fs::set_permissions(curl, fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = command(home.path()).unwrap();
    command
        .env("PATH", home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    assert!(run(command, &CancellationToken::new()).await.is_err());
    assert!(!home.path().join("executed").exists());
}

// Covers: cancellation and future drop kill the installer and its descendants.
// Owner: OS process lifecycle; inherited socket EOF proves tree cleanup.
#[tokio::test]
async fn installer_tree_stops_on_cancel_and_drop() {
    for abort in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let socket = home.path().join("started.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-c", "import os,socket,sys\ns=socket.socket(socket.AF_UNIX)\ns.connect(sys.argv[1])\nif os.fork()==0:\n s.sendall(b'ready')\n s.recv(1)\nelse:\n os.wait()\n"]).arg(socket).stdout(Stdio::null()).stderr(Stdio::null());
        let cancellation = Arc::new(CancellationToken::new());
        let token = cancellation.clone();
        let task = tokio::spawn(async move { run(command, &token).await });
        // Same process-start tripwire as the neighboring Cua MCP fixture tests.
        let (mut signal, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut ready = [0; 5];
        tokio::time::timeout(Duration::from_secs(10), signal.read_exact(&mut ready))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&ready, b"ready");
        if abort {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            cancellation.cancel();
            assert!(task.await.unwrap().is_err());
        }
        let mut remaining = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), signal.read_to_end(&mut remaining))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remaining, Vec::<u8>::new());
    }
}

use std::sync::Arc;
