//! Opt-in Cua desktop access. Only the host may connect; model calls never do.
//!
//! User activation and trusted driver configuration grant desktop-wide access,
//! not a workspace sandbox. MCP owns transport, budgets, results and cancellation.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use anyhow::{anyhow, bail};
use rho_sdk::{tool::Tool, CancellationToken};

use super::{
    mcp::{
        self,
        config::{McpConfig, McpSamplingPolicy, McpServerConfig, McpToolFilter, McpTransport},
    },
    sdk_registry::ToolBundle,
};

mod native;
mod policy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComputerUseStatus {
    Off,
    Connecting,
    Connected,
}

#[derive(Clone)]
pub(crate) struct ComputerUseSession {
    inner: Arc<Inner>,
}

struct Inner {
    driver: Option<PathBuf>,
    max_output_bytes: usize,
    cwd: PathBuf,
    state: Mutex<State>,
    // One desktop action at a time, including across cloned runtime handles.
    operation: tokio::sync::Mutex<()>,
}

struct State {
    status: ComputerUseStatus,
    cancellation: CancellationToken,
    connection: Option<Arc<Connection>>,
    pending: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

struct Connection {
    bundle: mcp::McpBundle,
    tools: BTreeMap<String, Arc<dyn Tool>>,
    instructions: Option<String>,
}

impl ComputerUseSession {
    pub(crate) fn new(driver: Option<PathBuf>, max_output_bytes: usize, cwd: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                driver,
                max_output_bytes,
                cwd,
                state: Mutex::new(State {
                    status: ComputerUseStatus::Off,
                    cancellation: CancellationToken::new(),
                    connection: None,
                    pending: None,
                }),
                operation: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub(crate) fn status(&self) -> ComputerUseStatus {
        self.state().status
    }

    /// Detection does not launch the driver or alter the desktop.
    pub(crate) fn driver_path(&self) -> Option<PathBuf> {
        if let Some(path) = &self.inner.driver {
            return Some(if path.is_absolute() {
                path.clone()
            } else {
                self.inner.cwd.join(path)
            });
        }
        detect_driver()
    }

    pub(crate) fn setup_guidance() -> &'static str {
        "Install Cua Driver separately using https://docs.cua.ai/ and make cua-driver available on PATH or at ~/.local/bin/cua-driver. Grant required desktop permissions in Cua's setup. Rho does not install, update, or bypass OS permissions. Use /computer on to grant desktop access for this session, including signed-in apps. Screenshots are sent to the model provider and persisted in session history."
    }

    /// Start a host-authorized connection without blocking the UI. The manager
    /// owns its task so every revocation path also stops pending activation.
    pub(crate) fn start_connect(&self) {
        let mut state = self.state();
        if state.pending.is_some() || state.status == ComputerUseStatus::Connected {
            return;
        }
        let session = self.clone();
        state.pending = Some(tokio::spawn(async move { session.connect().await }));
    }

    pub(crate) fn connection_pending(&self) -> bool {
        self.state().pending.is_some()
    }

    pub(crate) async fn take_connect_result(&self) -> Option<anyhow::Result<()>> {
        let task = self
            .state()
            .pending
            .take_if(|handle| handle.is_finished())?;
        Some(match task.await {
            Ok(result) => result,
            Err(error) => Err(error.into()),
        })
    }

    /// Host-only activation. Never called from a tool, retry path, or startup.
    pub(crate) async fn connect(&self) -> anyhow::Result<()> {
        if self.status() == ComputerUseStatus::Connected {
            return Ok(());
        }
        // Never queue a host grant behind an old action: an intervening off
        // must not be undone by a previously requested on.
        let _operation = self.inner.operation.try_lock().map_err(|_| anyhow!("computer operation is still closing or connecting; try /computer on after it finishes"))?;
        let driver = self
            .driver_path()
            .ok_or_else(|| anyhow!(Self::setup_guidance()))?;
        let cancellation = CancellationToken::new();
        {
            let mut state = self.state();
            state.status = ComputerUseStatus::Connecting;
            state.cancellation = cancellation.clone();
        }
        // A dropped connect future revokes its grant too.
        let mut guard = RevokeOnDrop::new(self.clone());
        let config = McpConfig {
            servers: BTreeMap::from([(
                "cua".into(),
                McpServerConfig {
                    enabled: true,
                    tools: McpToolFilter {
                        allow: policy::ALLOWED_TOOLS
                            .iter()
                            .map(|name| (*name).into())
                            .collect(),
                        deny: Vec::new(),
                    },
                    log_level: None,
                    sampling: McpSamplingPolicy::Deny,
                    transport: McpTransport::Stdio {
                        command: driver.to_string_lossy().into_owned(),
                        args: vec!["mcp".into()],
                        cwd: Some(self.inner.cwd.clone()),
                        env: BTreeMap::new(),
                        env_from_env: policy::desktop_environment(),
                    },
                    filesystem: None,
                },
            )]),
            invalid_servers: Vec::new(),
        };
        let options = mcp::McpSessionOptions::new(
            self.inner.max_output_bytes,
            mcp::McpRoots::default(),
            mcp::McpAuthorizationMode::NonInteractive,
        );
        let outcome = tokio::select! {
            biased;
            _ = cancellation.cancelled() => bail!("computer access was disabled during connection"),
            outcome = mcp::McpBundle::connect(&config, options) => outcome,
        };
        let report = outcome
            .report
            .servers
            .first()
            .ok_or_else(|| anyhow!("Cua returned no connection report"))?;
        let bundle = outcome.bundle.ok_or_else(|| {
            anyhow!(
                "could not connect Cua Driver: {}",
                report.error().unwrap_or("no live transport")
            )
        })?;
        let tools = report
            .tools()
            .iter()
            .filter_map(|entry| {
                bundle
                    .tools()
                    .iter()
                    .find(|tool| tool.spec().name == entry.exported_name)
                    .map(|tool| (entry.remote_name.clone(), tool.clone()))
            })
            .collect();
        let connection = Arc::new(Connection {
            bundle,
            tools,
            instructions: report.instructions().map(str::to_owned),
        });
        {
            let mut state = self.state();
            if cancellation.is_cancelled() {
                close_owned(connection);
                bail!("computer access was disabled during connection");
            }
            state.connection = Some(connection);
            state.status = ComputerUseStatus::Connected;
        }
        guard.armed = false;
        Ok(())
    }

    /// Revoke first, then close only this session's MCP child transport.
    /// Shutdown runs independently so dropping this future cannot abandon it.
    pub(crate) async fn disconnect(&self) {
        if let Some(task) = self.state().pending.take() {
            task.abort();
        }
        if let Some(task) = self.revoke() {
            let _ = task.await;
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn revoke(&self) -> Option<tokio::task::JoinHandle<()>> {
        let mut state = self.state();
        state.status = ComputerUseStatus::Off;
        state.cancellation.cancel();
        state.connection.take().map(close_owned)
    }

    pub(crate) fn tool(&self) -> Arc<dyn Tool> {
        Arc::new(native::ComputerTool(self.clone()))
    }
}

/// Read-only installation detection, independent of session construction.
pub(crate) fn detect_driver() -> Option<PathBuf> {
    policy::detect_driver(
        std::env::var_os("PATH"),
        crate::paths::home_dir().map(PathBuf::into_os_string),
    )
}

fn close_owned(connection: Arc<Connection>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        connection.bundle.shutdown().await;
    })
}

/// A cancelled/dropped action has an uncertain desktop effect. Fail closed and
/// require a new user grant rather than let the next action race that effect.
struct RevokeOnDrop {
    session: ComputerUseSession,
    armed: bool,
}
impl RevokeOnDrop {
    fn new(session: ComputerUseSession) -> Self {
        Self {
            session,
            armed: true,
        }
    }
}
impl Drop for RevokeOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.session.revoke();
        }
    }
}

#[cfg(test)]
#[path = "computer_use/computer_use_tests.rs"]
mod tests;
