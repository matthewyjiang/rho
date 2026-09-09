#![cfg(unix)]

use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt;

use super::*;

// Covers: caller opt-in cannot reach downloaded code, and setup must persist
// opt-out using the newly installed executable, propagating disable failures.
// Owner: installer process boundary; curl and the installed driver are fixtures.
#[tokio::test]
async fn installation_forces_telemetry_off_and_persists_before_success() {
    for disable_exit in [0, 7] {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let curl = home.path().join("curl");
        fs::write(&curl, "#!/bin/sh\nexec /bin/cat \"$HOME/install.sh\"\n").unwrap();
        fs::set_permissions(curl, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            home.path().join("install.sh"),
            r#"set -eu
[ "$CUA_DRIVER_RS_TELEMETRY_ENABLED" = false ]
[ "$CUA_TELEMETRY_ENABLED" = false ]
/bin/cp "$HOME/driver" "$HOME/.local/bin/cua-driver"
"#,
        )
        .unwrap();
        let driver = home.path().join("driver");
        fs::write(
            &driver,
            format!(
                r#"#!/bin/sh
set -eu
[ "$CUA_DRIVER_RS_TELEMETRY_ENABLED" = false ]
[ "$CUA_TELEMETRY_ENABLED" = false ]
[ "$1" = telemetry ]
[ "$2" = disable ]
echo disable-invoked
printf false > "$HOME/persisted"
exit {disable_exit}
"#
            ),
        )
        .unwrap();
        fs::set_permissions(driver, fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = command(home.path()).unwrap();
        command
            .env("PATH", home.path())
            .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "true")
            .env("CUA_TELEMETRY_ENABLED", "true");
        let (command, log) = with_log(command).unwrap();
        let result = run(command, &CancellationToken::new()).await;
        assert_eq!(result.is_ok(), disable_exit == 0);
        assert_eq!(
            fs::read_to_string(home.path().join("persisted")).unwrap(),
            "false"
        );
        assert_eq!(fs::read_to_string(&log).unwrap(), "disable-invoked\n");
        fs::remove_file(log).unwrap();
    }
}

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

// Covers: cancellation and future drop also kill the post-install telemetry
// command and its descendants, rather than treating the installer exit as done.
// Owner: OS process lifecycle; inherited socket EOF proves tree cleanup.
#[tokio::test]
async fn installer_tree_stops_on_cancel_and_drop() {
    for abort in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let socket = home.path().join("started.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let bin = home.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let driver = bin.join("cua-driver");
        fs::write(
            &driver,
            r#"#!/usr/bin/python3
import os, pathlib, socket, sys
assert sys.argv[1:] == ['telemetry', 'disable']
assert os.environ['CUA_DRIVER_RS_TELEMETRY_ENABLED'] == 'false'
assert os.environ['CUA_TELEMETRY_ENABLED'] == 'false'
s = socket.socket(socket.AF_UNIX)
s.connect(str(pathlib.Path(__file__).parent.parent.parent / 'started.sock'))
if os.fork() == 0:
    s.sendall(b'ready')
    s.recv(1)
else:
    os.wait()
"#,
        )
        .unwrap();
        fs::set_permissions(driver, fs::Permissions::from_mode(0o700)).unwrap();
        let curl = home.path().join("curl");
        fs::write(&curl, "#!/bin/sh\nprintf 'true\\n'\n").unwrap();
        fs::set_permissions(curl, fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = command(home.path()).unwrap();
        command
            .env("PATH", home.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
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
