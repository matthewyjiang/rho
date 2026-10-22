//! Shell commands in host terminals over `terminal/*`.
//!
//! A host that advertises `clientCapabilities.terminal`, such as Zed, runs
//! each `bash` call in a terminal it shows live on the call's card. Rho keeps
//! the policy: the command's approval, workspace checks, timeout, and the
//! model's result (see `rho_tools::HostTerminals`). This module only sends
//! the requests, and owns each terminal's lifetime.
//!
//! ACP requires a terminal to be on a card before it is released, and Zed
//! rejects a card update that names a released terminal. So a terminal is
//! released only once its call's final card update has gone out: at the end
//! of the prompt it ran in, or for one still running then, such as a
//! delegated agent's, at the end of the first prompt after it finished.
//! Closing the session releases every terminal, which kills those still
//! running.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use agent_client_protocol::{
    schema::v1::{
        CreateTerminalRequest, CreateTerminalResponse, EnvVariable, KillTerminalRequest,
        ReleaseTerminalRequest, SessionId, TerminalId, TerminalOutputRequest,
        TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
    },
    Client, ConnectionTo, Error as AcpError,
};
use rho_tools::{
    HostCommand, HostError, HostExitStatus, HostFuture, HostTerminal, HostTerminalOutput,
    HostTerminals,
};

use super::{AcpClientPort, AcpStartup};

/// One `terminal/*` request.
pub(in crate::app) enum TerminalRequest {
    Create(CreateTerminalRequest),
    Output(TerminalOutputRequest),
    WaitForExit(WaitForTerminalExitRequest),
    Kill(KillTerminalRequest),
    Release(ReleaseTerminalRequest),
}

/// The host's answer to a [`TerminalRequest`] of the same kind.
pub(in crate::app) enum TerminalResponse {
    Created(CreateTerminalResponse),
    Output(TerminalOutputResponse),
    Exited(WaitForTerminalExitResponse),
    Killed,
    Released,
}

/// Sends `request` on the connection.
pub(super) async fn send(
    cx: &ConnectionTo<Client>,
    request: TerminalRequest,
) -> Result<TerminalResponse, AcpError> {
    Ok(match request {
        TerminalRequest::Create(request) => {
            TerminalResponse::Created(cx.send_request(request).block_task().await?)
        }
        TerminalRequest::Output(request) => {
            TerminalResponse::Output(cx.send_request(request).block_task().await?)
        }
        TerminalRequest::WaitForExit(request) => {
            TerminalResponse::Exited(cx.send_request(request).block_task().await?)
        }
        TerminalRequest::Kill(request) => {
            cx.send_request(request).block_task().await?;
            TerminalResponse::Killed
        }
        TerminalRequest::Release(request) => {
            cx.send_request(request).block_task().await?;
            TerminalResponse::Released
        }
    })
}

/// Whether Rho still holds a host terminal's handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lease {
    Running,
    /// The call ended; the terminal waits for release.
    Finished,
}

/// One session's host terminals not yet released.
#[derive(Default)]
struct Leases(std::sync::Mutex<HashMap<TerminalId, Lease>>);

impl Leases {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<TerminalId, Lease>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One session's host terminals. The session's `ClientIo` holds the only
/// instance, so terminals every tool set of the session starts, including
/// a delegated agent's, are released by the session.
pub(super) struct ClientTerminals {
    port: Arc<dyn AcpClientPort>,
    session_id: SessionId,
    leases: Arc<Leases>,
}

impl ClientTerminals {
    /// Session `session_id`'s terminals on `startup`'s host. `None` unless
    /// the host advertised `terminal` at `initialize`.
    pub(super) fn for_session(startup: &AcpStartup, session_id: &SessionId) -> Option<Arc<Self>> {
        let port = startup
            .client
            .as_ref()
            .filter(|_| startup.host_features.terminals)?;
        Some(Arc::new(Self {
            port: Arc::clone(port),
            session_id: session_id.clone(),
            leases: Arc::default(),
        }))
    }

    /// Releases the terminals whose calls ended. Call it only after every
    /// card update for those calls was sent.
    pub(super) async fn release_finished(&self) {
        self.release(/*including_running*/ false).await;
    }

    /// Releases every terminal, which stops commands still running.
    pub(super) async fn release_all(&self) {
        self.release(/*including_running*/ true).await;
    }

    async fn release(&self, including_running: bool) {
        let released: Vec<TerminalId> = {
            let mut leases = self.leases.lock();
            let ids = leases
                .iter()
                .filter(|(_, lease)| including_running || **lease == Lease::Finished)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            for id in &ids {
                leases.remove(id);
            }
            ids
        };
        for id in released {
            let request = ReleaseTerminalRequest::new(self.session_id.clone(), id.clone());
            if let Err(error) = self.port.terminal(TerminalRequest::Release(request)).await {
                tracing::warn!(terminal = %id.0, %error, "could not release a host terminal");
            }
        }
    }
}

impl HostTerminals for ClientTerminals {
    fn create(&self, command: HostCommand) -> HostFuture<'_, Box<dyn HostTerminal>> {
        Box::pin(async move {
            let request = CreateTerminalRequest::new(self.session_id.clone(), command.program)
                .args(command.args)
                .env(
                    command
                        .env
                        .into_iter()
                        .map(|(name, value)| EnvVariable::new(name, value))
                        .collect(),
                )
                .cwd(command.cwd)
                .output_byte_limit(command.output_byte_limit);
            let (port, session_id, leases) = (
                Arc::clone(&self.port),
                self.session_id.clone(),
                Arc::clone(&self.leases),
            );
            let (claim, claimed) = tokio::sync::oneshot::channel();
            // Spawned, so the host's reply is taken even when the call was
            // cancelled before it arrived. A terminal no call claims is
            // released at once, which stops its command.
            tokio::spawn(async move {
                let created = match port.terminal(TerminalRequest::Create(request)).await {
                    Ok(TerminalResponse::Created(created)) => Ok(created),
                    Ok(_) => Err(mismatched("terminal/create")),
                    Err(error) => Err(host_error(error)),
                };
                let _ = claim.send(created.map(|created| {
                    Unclaimed(Some(ClientTerminal {
                        port,
                        session_id,
                        id: created.terminal_id,
                        leases,
                        running: AtomicBool::new(true),
                    }))
                }));
            });
            let terminal = claimed
                .await
                .map_err(|_| HostError::new("terminal/create task ended without a reply"))??
                .claim();
            self.leases
                .lock()
                .insert(terminal.id.clone(), Lease::Running);
            Ok(Box::new(terminal) as Box<dyn HostTerminal>)
        })
    }
}

/// A created terminal no call has taken yet. Dropped unclaimed, because the
/// call that asked for it was cancelled, it releases the terminal.
struct Unclaimed(Option<ClientTerminal>);

impl Unclaimed {
    fn claim(mut self) -> ClientTerminal {
        self.0.take().expect("a terminal is claimed once")
    }
}

impl Drop for Unclaimed {
    fn drop(&mut self) {
        let Some(terminal) = self.0.take() else {
            return;
        };
        // Releasing stops the command, so the handle has nothing to kill.
        terminal.running.store(false, Ordering::Relaxed);
        let request = ReleaseTerminalRequest::new(terminal.session_id.clone(), terminal.id.clone());
        terminal.send_detached(TerminalRequest::Release(request), "release");
    }
}

/// Rho's handle on one host terminal. Dropping it marks the terminal ready
/// for release, and kills a command it has not seen exit: the SDK drops a
/// cancelled call's future without letting it kill the command itself.
struct ClientTerminal {
    port: Arc<dyn AcpClientPort>,
    session_id: SessionId,
    id: TerminalId,
    leases: Arc<Leases>,
    /// Whether the command may still be running: cleared once it is seen to
    /// exit or is killed.
    running: AtomicBool,
}

impl ClientTerminal {
    async fn send(&self, request: TerminalRequest) -> Result<TerminalResponse, HostError> {
        self.port.terminal(request).await.map_err(host_error)
    }

    /// Sends `request` without waiting for the reply, for cleanup on drop.
    fn send_detached(&self, request: TerminalRequest, method: &'static str) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(terminal = %self.id.0, method, "no runtime to send a host terminal request");
            return;
        };
        let (port, id) = (Arc::clone(&self.port), self.id.clone());
        runtime.spawn(async move {
            if let Err(error) = port.terminal(request).await {
                tracing::warn!(terminal = %id.0, method, %error, "host terminal request failed");
            }
        });
    }
}

impl HostTerminal for ClientTerminal {
    fn id(&self) -> &str {
        &self.id.0
    }

    fn wait_for_exit(&self) -> HostFuture<'_, HostExitStatus> {
        Box::pin(async move {
            let request = WaitForTerminalExitRequest::new(self.session_id.clone(), self.id.clone());
            match self.send(TerminalRequest::WaitForExit(request)).await? {
                TerminalResponse::Exited(exited) => {
                    self.running.store(false, Ordering::Relaxed);
                    Ok(exit_status(exited.exit_status))
                }
                _ => Err(mismatched("terminal/wait_for_exit")),
            }
        })
    }

    fn output(&self) -> HostFuture<'_, HostTerminalOutput> {
        Box::pin(async move {
            let request = TerminalOutputRequest::new(self.session_id.clone(), self.id.clone());
            match self.send(TerminalRequest::Output(request)).await? {
                TerminalResponse::Output(output) => {
                    Ok(HostTerminalOutput::new(output.output, output.truncated))
                }
                _ => Err(mismatched("terminal/output")),
            }
        })
    }

    fn kill(&self) -> HostFuture<'_, ()> {
        Box::pin(async move {
            let request = KillTerminalRequest::new(self.session_id.clone(), self.id.clone());
            self.send(TerminalRequest::Kill(request)).await?;
            self.running.store(false, Ordering::Relaxed);
            Ok(())
        })
    }
}

impl Drop for ClientTerminal {
    fn drop(&mut self) {
        if let Some(lease) = self.leases.lock().get_mut(&self.id) {
            *lease = Lease::Finished;
        }
        if self.running.load(Ordering::Relaxed) {
            let request = KillTerminalRequest::new(self.session_id.clone(), self.id.clone());
            self.send_detached(TerminalRequest::Kill(request), "kill");
        }
    }
}

/// ACP's exit code is unsigned; one that does not fit an `i32` is reported
/// as no code, the same as a signal.
fn exit_status(status: agent_client_protocol::schema::v1::TerminalExitStatus) -> HostExitStatus {
    HostExitStatus::new(
        status.exit_code.and_then(|code| i32::try_from(code).ok()),
        status.signal,
    )
}

fn host_error(error: AcpError) -> HostError {
    HostError::new(error.to_string())
}

fn mismatched(method: &str) -> HostError {
    HostError::new(format!("{method} got a reply of another kind"))
}

#[cfg(test)]
#[path = "client_terminals_tests.rs"]
mod tests;
