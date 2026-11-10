//! Requests the TUI sends about the session without waiting for its turn:
//! each goes to the session's request queue, and what it answers with
//! arrives as a view.

use super::{DaemonSession, LOST};

impl DaemonSession {
    /// Request live inventory without waiting for the active turn.
    pub(in crate::tui) fn request_mcp(&self) -> anyhow::Result<()> {
        if self.ended {
            anyhow::bail!(LOST);
        }
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .request_mcp()
    }

    /// Send a goal action without waiting for the active turn.
    pub(in crate::tui) fn goal(
        &self,
        action: rho_protocol::SessionGoalAction,
    ) -> anyhow::Result<()> {
        if self.ended {
            anyhow::bail!(LOST);
        }
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .goal(action)
    }

    /// Ask for the daemon's jobs without waiting for the active turn;
    /// they arrive as `DaemonView::Jobs`.
    pub(in crate::tui) fn request_jobs(&self) -> anyhow::Result<()> {
        if self.ended {
            anyhow::bail!(LOST);
        }
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .request_jobs()
    }

    /// Rename the session without waiting for the active turn; its
    /// `TitleChanged` event reports the new title.
    pub(in crate::tui) fn rename(&self, title: String) -> anyhow::Result<()> {
        if self.ended {
            anyhow::bail!(LOST);
        }
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .rename(title)
    }

    /// Writes settings without waiting for the active turn; a refusal
    /// arrives as a view, and the change itself as `SettingsChanged`.
    pub(in crate::tui) fn set_config(
        &self,
        params: rho_protocol::ConfigSetParams,
        label: String,
    ) -> anyhow::Result<()> {
        if self.ended {
            anyhow::bail!(LOST);
        }
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .set_config(params, label)
    }

    /// Queue a change without waiting for the active turn to release it.
    pub(in crate::tui) fn configure(
        &self,
        change: rho_protocol::SessionChange,
    ) -> anyhow::Result<()> {
        if self.ended {
            anyhow::bail!(LOST);
        }
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .configure(change)
    }
}
