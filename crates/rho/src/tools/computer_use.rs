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
use futures_util::{
    future::{BoxFuture, Shared},
    FutureExt,
};
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
mod recovery;
mod setup;
use recovery::{Revocation, RevokeOnDrop};
pub(crate) use setup::{setup_platform, ComputerSetupUpdate, INSTALLATION_RECOVERY};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComputerUseStatus {
    Off,
    Installing,
    Connecting,
    Connected,
    Closing,
}

#[derive(Clone)]
pub(crate) struct ComputerUseSession {
    inner: Arc<Inner>,
}

/// The TUI can inspect and revoke authority while a turn borrows the runtime.
/// Activation and transport task ownership remain with the runtime's session.
#[derive(Clone)]
pub(crate) struct ComputerUseControl(ComputerUseSession);

impl ComputerUseControl {
    pub(crate) fn installation_pending(&self) -> bool {
        self.0.installation_pending()
    }

    pub(crate) fn revoke(&self) {
        self.0.revoke();
    }

    pub(crate) fn status(&self) -> ComputerUseStatus {
        self.0.status()
    }

    pub(crate) fn driver_path(&self) -> Option<PathBuf> {
        self.0.driver_path()
    }

    pub(crate) fn terminal_error(&self) -> Option<String> {
        self.0.terminal_error()
    }

    pub(crate) fn revocation_reason(&self) -> Option<String> {
        self.0.revocation_reason()
    }
}

struct Inner {
    driver: Option<PathBuf>,
    max_output_bytes: usize,
    cwd: PathBuf,
    state: Mutex<State>,
    // One desktop action at a time, including across cloned runtime handles.
    operation: tokio::sync::Mutex<()>,
}

type Task<T> = Shared<BoxFuture<'static, Result<T, Arc<str>>>>;

enum State {
    Installing(setup::Installation),
    Off {
        error: Option<Arc<str>>,
        revocation: Option<Revocation>,
    },
    Connecting {
        grant: Arc<CancellationToken>,
        task: Task<Arc<Connection>>,
    },
    Connected {
        grant: Arc<CancellationToken>,
        connection: Arc<Connection>,
    },
    Closing {
        grant: Arc<CancellationToken>,
        task: Task<()>,
        revocation: Option<Revocation>,
    },
}

struct Connection {
    bundle: mcp::McpBundle,
    tools: BTreeMap<String, Arc<dyn Tool>>,
    instructions: Option<String>,
}

impl ComputerUseSession {
    pub(crate) fn control(&self) -> ComputerUseControl {
        ComputerUseControl(self.clone())
    }

    pub(crate) fn new(driver: Option<PathBuf>, max_output_bytes: usize, cwd: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                driver,
                max_output_bytes,
                cwd,
                state: Mutex::new(State::Off {
                    error: None,
                    revocation: None,
                }),
                operation: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub(crate) fn status(&self) -> ComputerUseStatus {
        match &*self.state() {
            State::Installing(_) => ComputerUseStatus::Installing,
            State::Off { .. } => ComputerUseStatus::Off,
            State::Connecting { .. } => ComputerUseStatus::Connecting,
            State::Connected { .. } => ComputerUseStatus::Connected,
            State::Closing { .. } => ComputerUseStatus::Closing,
        }
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

    /// Start a host-authorized connection without blocking the UI. The manager
    /// owns its task so every revocation path also stops pending activation.
    pub(crate) fn start_connect(&self) -> anyhow::Result<()> {
        let mut state = self.state();
        match &*state {
            State::Installing(_) => {
                bail!("wait for Cua Driver installation to finish before granting desktop access")
            }
            State::Connected { .. } | State::Connecting { .. } => return Ok(()),
            State::Closing { .. } => {
                bail!("computer transport is still closing; try /computer on after it finishes")
            }
            State::Off { .. } => {}
        }
        let grant = Arc::new(CancellationToken::new());
        let session = self.clone();
        let cancellation = grant.clone();
        let task = retained_task(async move { session.open_connection(&cancellation).await });
        *state = State::Connecting { grant, task };
        Ok(())
    }

    pub(crate) async fn take_connect_result(&self) -> Option<anyhow::Result<()>> {
        let (grant, task) = match &*self.state() {
            State::Connecting { grant, task } => (grant.clone(), task.clone()),
            _ => return None,
        };
        let result = task.now_or_never()?;
        Some(self.finish_connect(&grant, result))
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_connect_result(&self) {
        let task = match &*self.state() {
            State::Connecting { task, .. } => task.clone(),
            _ => panic!("expected a pending connection"),
        };
        let _ = task.await;
    }

    /// Host-only activation. Never called from a tool, retry path, or startup.
    #[cfg(test)]
    pub(crate) async fn connect(&self) -> anyhow::Result<()> {
        self.start_connect()?;
        let (grant, task) = match &*self.state() {
            State::Connecting { grant, task } => (grant.clone(), task.clone()),
            State::Connected { .. } => return Ok(()),
            State::Off { .. } | State::Installing(_) | State::Closing { .. } => {
                bail!("computer access was disabled during connection")
            }
        };
        let mut guard = RevokeOnDrop::new(self.clone(), grant.clone());
        let result = self.finish_connect(&grant, task.await);
        guard.disarm();
        result
    }

    fn finish_connect(
        &self,
        grant: &Arc<CancellationToken>,
        result: Result<Arc<Connection>, Arc<str>>,
    ) -> anyhow::Result<()> {
        let mut state = self.state();
        if !matches!(&*state, State::Connecting { grant: current, .. } if Arc::ptr_eq(current, grant))
        {
            bail!("computer access was disabled during connection");
        }
        match result {
            Ok(connection) => {
                *state = State::Connected {
                    grant: grant.clone(),
                    connection,
                };
                Ok(())
            }
            Err(error) => {
                *state = State::Off {
                    error: Some(error.clone()),
                    revocation: None,
                };
                Err(anyhow!(error.to_string()))
            }
        }
    }

    async fn open_connection(
        &self,
        cancellation: &CancellationToken,
    ) -> anyhow::Result<Arc<Connection>> {
        let driver = self
            .driver_path()
            .ok_or_else(|| anyhow!("Cua Driver was not found; use /computer setup for installation and permission guidance"))?;
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
                        env: policy::driver_environment(),
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
        )
        .with_image_delivery(mcp::McpImageDelivery::ModelAndPresentation);
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
        Ok(connection)
    }

    /// Revoke first, then close only this session's MCP child transport.
    /// Shutdown runs independently so dropping this future cannot abandon it.
    pub(crate) async fn disconnect(&self) {
        self.revoke();
        self.finish_installation().await;
        self.finish_closing(/*wait*/ true).await;
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn revoke(&self) {
        self.revoke_grant(/*expected*/ None, /*revocation*/ None);
    }

    fn revoke_grant(
        &self,
        expected: Option<&Arc<CancellationToken>>,
        revocation: Option<Revocation>,
    ) {
        let mut state = self.state();
        let grant = match &*state {
            State::Installing(installation) => {
                if expected.is_none() {
                    installation.cancel();
                }
                return;
            }
            State::Connecting { grant, .. } | State::Connected { grant, .. } => grant.clone(),
            State::Off { .. } | State::Closing { .. } => return,
        };
        if expected.is_some_and(|expected| !Arc::ptr_eq(expected, &grant)) {
            return;
        }
        grant.cancel();
        let previous = std::mem::replace(
            &mut *state,
            State::Off {
                error: None,
                revocation: None,
            },
        );
        let session = self.clone();
        let task = retained_task(async move {
            let connection = match previous {
                // Revoking activation normally cancels before a connection is
                // produced. That is completed cleanup, not a lifecycle failure.
                State::Connecting { task, .. } => task.await.ok(),
                State::Connected { connection, .. } => Some(connection),
                State::Off { .. } | State::Installing(_) | State::Closing { .. } => unreachable!(),
            };
            // Revocation is immediate, but cleanup must also wait for an old
            // action to release its transport before accepting another grant.
            let _operation = session.inner.operation.lock().await;
            if let Some(connection) = connection {
                connection.bundle.shutdown().await;
            }
            Ok(())
        });
        *state = State::Closing {
            grant,
            task,
            revocation,
        };
    }

    pub(crate) async fn finish_closing(&self, wait: bool) {
        let (grant, task) = match &*self.state() {
            State::Closing { grant, task, .. } => (grant.clone(), task.clone()),
            _ => return,
        };
        let result = if wait {
            Some(task.await)
        } else {
            task.now_or_never()
        };
        if let Some(result) = result {
            let mut state = self.state();
            match &mut *state {
                State::Closing {
                    grant: current,
                    revocation,
                    ..
                } if Arc::ptr_eq(current, &grant) => {
                    let revocation = revocation.take();
                    *state = State::Off {
                        error: result.err(),
                        revocation,
                    };
                }
                State::Off { .. }
                | State::Installing(_)
                | State::Connecting { .. }
                | State::Connected { .. }
                | State::Closing { .. } => {}
            }
        }
    }

    pub(crate) fn terminal_error(&self) -> Option<String> {
        match &*self.state() {
            State::Off { error, .. } => error.as_ref().map(ToString::to_string),
            _ => None,
        }
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
        std::env::var_os("LOCALAPPDATA"),
    )
}

/// Read-only environment diagnostic, not a desktop permission probe.
pub(crate) fn desktop_warning() -> Option<&'static str> {
    if cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none_or(|value| value.is_empty())
    {
        Some("DISPLAY is unset or empty; Cua's X11 overlay cannot connect. A Wayland session may still be available, but desktop access has not been verified. Launch Rho from your desktop session with its display environment; do not guess a DISPLAY value.")
    } else {
        None
    }
}

fn retained_task<T: Clone + Send + Sync + 'static>(
    future: impl std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
) -> Task<T> {
    let task = tokio::spawn(future);
    async move {
        task.await
            .map_err(|error| Arc::from(error.to_string()))?
            .map_err(|error| Arc::from(error.to_string()))
    }
    .boxed()
    .shared()
}

#[cfg(test)]
#[path = "computer_use/computer_use_tests.rs"]
mod tests;
