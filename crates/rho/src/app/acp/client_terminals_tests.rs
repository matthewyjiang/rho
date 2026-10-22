use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{
    schema::v1::{
        CreateTerminalResponse, RequestPermissionRequest, RequestPermissionResponse,
        SessionNotification,
    },
    Error as AcpError,
};
use pretty_assertions::assert_eq;
use tokio::sync::Notify;

use super::*;

/// A host that answers `terminal/create` only once `reply` is notified, and
/// records the terminals it is asked to release or kill, notifying `release`
/// or `kill`.
#[derive(Default)]
struct SlowHost {
    sent: Notify,
    reply: Notify,
    release: Notify,
    released: Mutex<Vec<String>>,
    kill: Notify,
    killed: Mutex<Vec<String>>,
}

impl AcpClientPort for SlowHost {
    fn send_session_notification(
        &self,
        _notification: SessionNotification,
    ) -> Pin<Box<dyn Future<Output = Result<(), AcpError>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RequestPermissionResponse, AcpError>> + Send + '_>>
    {
        Box::pin(async { Err(AcpError::method_not_found()) })
    }

    fn terminal(
        &self,
        request: TerminalRequest,
    ) -> Pin<Box<dyn Future<Output = Result<TerminalResponse, AcpError>> + Send + '_>> {
        Box::pin(async move {
            match request {
                TerminalRequest::Create(_) => {
                    self.sent.notify_one();
                    self.reply.notified().await;
                    Ok(TerminalResponse::Created(CreateTerminalResponse::new(
                        "term-1",
                    )))
                }
                TerminalRequest::Release(release) => {
                    self.released
                        .lock()
                        .unwrap()
                        .push(release.terminal_id.0.to_string());
                    self.release.notify_one();
                    Ok(TerminalResponse::Released)
                }
                TerminalRequest::Kill(kill) => {
                    self.killed
                        .lock()
                        .unwrap()
                        .push(kill.terminal_id.0.to_string());
                    self.kill.notify_one();
                    Ok(TerminalResponse::Killed)
                }
                _ => Err(AcpError::method_not_found()),
            }
        })
    }
}

fn session_terminals(host: &Arc<SlowHost>) -> (tempfile::TempDir, Arc<ClientTerminals>) {
    let home = tempfile::tempdir().unwrap();
    let mut startup = AcpStartup::for_tests(crate::config::Config::default(), home.path());
    startup.client = Some(host.clone());
    startup.host_features.terminals = true;
    let terminals = ClientTerminals::for_session(&startup, &SessionId::new("s")).unwrap();
    (home, terminals)
}

fn sleep_command() -> HostCommand {
    HostCommand::new(
        "env",
        vec!["bash".into(), "-c".into(), "sleep 100".into()],
        "/work",
        64_000,
    )
}

// Covers: a terminal the host creates after its call was cancelled is
// released as soon as the reply arrives, with no prompt end or session close
// left to sweep it, instead of leaving the command running unseen.
// Owner: ACP client terminal leases
#[tokio::test]
async fn a_terminal_created_after_cancellation_is_released() {
    let host = Arc::new(SlowHost::default());
    let (_home, terminals) = session_terminals(&host);

    {
        let create = terminals.create(sleep_command());
        tokio::pin!(create);
        tokio::select! {
            biased;
            () = host.sent.notified() => {}
            _ = &mut create => panic!("created before the host replied"),
        }
        // Dropping the create here is the cancelled call.
    }
    host.reply.notify_one();
    // A timeout means the terminal was never released.
    tokio::time::timeout(std::time::Duration::from_secs(30), host.release.notified())
        .await
        .expect("the unclaimed terminal is released");

    assert_eq!(
        (
            host.released.lock().unwrap().clone(),
            terminals.leases.lock().is_empty()
        ),
        (vec!["term-1".to_owned()], true)
    );
}

// Covers: a cancelled call drops its terminal handle mid-command, since the
// SDK drops the tool future; the host is told to kill the command right away
// rather than letting it run until the prompt ends.
// Owner: ACP client terminal handles
#[tokio::test]
async fn dropping_a_running_terminal_kills_its_command() {
    let host = Arc::new(SlowHost::default());
    let (_home, terminals) = session_terminals(&host);
    host.reply.notify_one();

    drop(terminals.create(sleep_command()).await.unwrap());
    // A timeout means the command was never killed.
    tokio::time::timeout(std::time::Duration::from_secs(30), host.kill.notified())
        .await
        .expect("the dropped terminal's command is killed");

    assert_eq!(
        (
            host.killed.lock().unwrap().clone(),
            terminals
                .leases
                .lock()
                .get(&TerminalId::new("term-1"))
                .copied()
        ),
        (vec!["term-1".to_owned()], Some(Lease::Finished))
    );
}
