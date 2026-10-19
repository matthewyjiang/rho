//! SIGINT/SIGTERM handling for headless runs.
//!
//! Handlers must exist before a run becomes observable. A lazily registered
//! handler leaves a window after the first output where the default
//! disposition kills the process without cleanup or a final event.

use std::{fmt, io};

/// The signal that stopped a headless run.
#[derive(Clone, Copy, Debug)]
pub(super) enum ShutdownSignal {
    Interrupt,
    #[cfg(unix)]
    Terminate,
}

impl ShutdownSignal {
    /// Conventional process exit code: 128 plus the signal number.
    pub(super) fn exit_code(self) -> u8 {
        match self {
            Self::Interrupt => 130,
            #[cfg(unix)]
            Self::Terminate => 143,
        }
    }
}

impl fmt::Display for ShutdownSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interrupt => formatter.write_str("SIGINT"),
            #[cfg(unix)]
            Self::Terminate => formatter.write_str("SIGTERM"),
        }
    }
}

/// Installed shutdown handlers. Create them before starting the run; signals
/// that arrive between `install` and `recv` are still delivered to `recv`.
#[cfg(unix)]
pub(super) struct ShutdownSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl ShutdownSignals {
    pub(super) fn install() -> io::Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};

        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    pub(super) async fn recv(&mut self) -> io::Result<ShutdownSignal> {
        tokio::select! {
            _ = self.interrupt.recv() => Ok(ShutdownSignal::Interrupt),
            _ = self.terminate.recv() => Ok(ShutdownSignal::Terminate),
        }
    }
}

#[cfg(not(unix))]
pub(super) struct ShutdownSignals;

#[cfg(not(unix))]
impl ShutdownSignals {
    pub(super) fn install() -> io::Result<Self> {
        Ok(Self)
    }

    pub(super) async fn recv(&mut self) -> io::Result<ShutdownSignal> {
        tokio::signal::ctrl_c().await?;
        Ok(ShutdownSignal::Interrupt)
    }
}
