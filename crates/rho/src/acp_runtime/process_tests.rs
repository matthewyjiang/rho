//! Unix process boundary only; protocol lifecycle lives in Channel tests.

use super::{run_session, test_support::*};
use crate::{cli_runtime::CliExecutable, run_artifacts::AttachmentEvent, subagent::RunState};
use pretty_assertions::assert_eq;
use std::{io::Read, os::unix::fs::OpenOptionsExt, path::Path};
use tokio::io::unix::AsyncFd;

fn shell_request(dir: &Path, body: &str) -> (super::AcpSessionRequest, TestPolicy) {
    let mut request = request(dir, None);
    request.overrides.executable = Some(CliExecutable::resolve("sh").expect("Unix shell"));
    let mut policy = TestPolicy::new(dir);
    policy.argv = vec!["-c".into(), body.into()];
    (request, policy)
}

// Covers: a child exiting before initialize must retain its startup stderr,
// not invent success or replace the agent diagnostic with a transport error.
// Owner: redirected-stderr/process teardown boundary.
#[tokio::test]
async fn pre_handshake_exit_keeps_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    let (request, policy) = shell_request(dir.path(), "printf 'startup refused\\n' >&2; exit 1");
    tokio::time::timeout(TEST_BUDGET, run_session(request, policy))
        .await
        .expect("startup failure completed")
        .unwrap();
    let (status, events) = read_artifacts(dir.path());
    assert_eq!(status.state, RunState::Error);
    let error = status.error.unwrap();
    assert!(
        error.ends_with("startup refused"),
        "stderr diagnostic missing: {error}"
    );
    assert_eq!(
        terminal_events(&events),
        vec![AttachmentEvent::Failed(error)]
    );
}

// Covers: a noncooperative agent cannot keep a process alive after cancellation.
// Owner: OwnedChild-backed ACP process supervision; readiness is a FIFO signal,
// not a sleep or a timing assumption about the handshake.
#[tokio::test]
async fn cancellation_reaps_a_hung_prompt_child() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("ready.fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    // O_RDWR opens without waiting for a writer. AsyncFd makes the wait
    // cancellable if the fake fails before reaching its prompt.
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)
        .unwrap();
    let reader = AsyncFd::new(reader).unwrap();
    let (request, mut policy) = shell_request(
        dir.path(),
        r#"
request_id() { printf '%s\n' "$1" | sed -n 's/.*"id":\([^,}]*\).*/\1/p'; }
IFS= read -r initialize
id=$(request_id "$initialize")
printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}\n' "$id"
IFS= read -r new_session
id=$(request_id "$new_session")
printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"hung-session"}}\n' "$id"
IFS= read -r prompt
printf '%s\n' "$$" > "$ACP_READY_FIFO"
exec tail -f /dev/null
"#,
    );
    policy
        .env
        .push(("ACP_READY_FIFO".into(), fifo.into_os_string()));
    let cancellation = request.cancellation.clone();
    let pid = tokio::time::timeout(TEST_BUDGET, async {
        let cancel = async {
            let mut line = Vec::new();
            loop {
                let mut guard = reader.readable().await.unwrap();
                let mut byte = [0_u8];
                match guard.try_io(|inner| inner.get_ref().read(&mut byte)) {
                    Ok(Ok(1)) if byte[0] == b'\n' => break,
                    Ok(Ok(1)) => line.push(byte[0]),
                    Ok(Ok(_)) => panic!("readiness FIFO closed before pid"),
                    Ok(Err(error)) => panic!("readiness FIFO: {error}"),
                    Err(_) => {}
                }
            }
            let pid = std::str::from_utf8(&line)
                .unwrap()
                .parse::<libc::pid_t>()
                .unwrap();
            cancellation.cancel();
            pid
        };
        let (run, pid) = tokio::join!(run_session(request, policy), cancel);
        run.unwrap();
        pid
    })
    .await
    .expect("hung agent cancelled and reaped");
    // SAFETY: signal 0 only checks existence of the reaped child's recorded pid.
    let exists = unsafe { libc::kill(pid, 0) };
    let error = std::io::Error::last_os_error().raw_os_error();
    assert_eq!((exists, error), (-1, Some(libc::ESRCH)));
    let (status, events) = read_artifacts(dir.path());
    assert_eq!(
        (
            status.state,
            status.claude_session_id,
            terminal_events(&events)
        ),
        (
            RunState::Stopped,
            Some("hung-session".into()),
            vec![AttachmentEvent::Cancelled]
        )
    );
}
