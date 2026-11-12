//! Requests the TUI sends about the session without waiting for its turn:
//! each goes to the session's request queue, and what it answers with
//! arrives as a view.

use super::{DaemonSession, Following, Target, ToolSources, LOST, NOT_OPENED};

impl DaemonSession {
    /// Whether the session's MCP servers still connect, so a prompt waits.
    /// While the daemon restarts nobody knows yet: the restarted one
    /// connects them again, so a prompt waits until the session attached
    /// there and read them.
    pub(in crate::tui) fn tool_sources_loading(&self) -> bool {
        self.attachment()
            .is_some_and(|attachment| attachment.tool_sources == ToolSources::Loading)
            || matches!(*self.target.borrow(), Target::Reconnecting)
    }

    /// Refuses a request about the session before it opened, and one
    /// once following ended.
    fn ensure_followed(&self) -> anyhow::Result<()> {
        match &self.following {
            Following::Draft(_) => anyhow::bail!(NOT_OPENED),
            Following::Attached(attachment) if attachment.ended => anyhow::bail!(LOST),
            Following::Attached(_) => Ok(()),
        }
    }

    /// Request live inventory without waiting for the active turn.
    pub(in crate::tui) fn request_mcp(&self) -> anyhow::Result<()> {
        self.ensure_followed()?;
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .request_mcp()
    }

    /// Ask for a report on the session, which arrives as
    /// `DaemonView::Report`; the daemon refuses during a turn.
    pub(in crate::tui) fn inspect(
        &self,
        report: rho_protocol::SessionReport,
    ) -> anyhow::Result<()> {
        self.ensure_followed()?;
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .inspect(report)
    }

    /// Reads what of the conversation the session's model cannot be sent,
    /// now: the send that asks waits on the answer, so it does not go
    /// through the request queue. The daemon refuses during a turn.
    pub(in crate::tui) async fn provider_context(
        &self,
    ) -> anyhow::Result<crate::app::provider_context_report::ProviderContextReport> {
        let session = self.opened_id()?;
        let answer = self
            .attached_link()
            .await?
            .request::<rho_protocol::methods::SessionInspect>(&rho_protocol::SessionInspectParams {
                session,
                report: rho_protocol::SessionReport::ProviderContext,
            })
            .await?;
        Ok(serde_json::from_value(answer.report)?)
    }

    /// Ask for the daemon's jobs without waiting for the active turn;
    /// they arrive as `DaemonView::Jobs`.
    pub(in crate::tui) fn request_jobs(&self) -> anyhow::Result<()> {
        if self.ended() {
            anyhow::bail!(LOST);
        }
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .request_jobs()
    }

    /// Ask for the session's processes, and one's output from a cursor on,
    /// without waiting for the active turn; they arrive as
    /// `DaemonView::Processes`.
    pub(in crate::tui) fn request_processes(
        &self,
        output: Option<rho_protocol::ProcessOutputParams>,
    ) -> anyhow::Result<()> {
        self.ensure_followed()?;
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .request_processes(output)
    }

    /// A blocking delete of saved sessions through the daemon, which can
    /// let go of one it holds open: one answer per id, in order. For the
    /// blocking pool only, never a runtime thread.
    pub(in crate::tui) fn deleter(&self) -> crate::tui::sessions_hub_tasks::DeleteHeld {
        let deleter = self
            .requests
            .as_ref()
            .expect("attached session has requests")
            .deleter();
        let runtime = tokio::runtime::Handle::current();
        Box::new(move |sessions| runtime.block_on(deleter.delete(sessions)))
    }

    /// Rename the session without waiting for the active turn; its
    /// `TitleChanged` event reports the new title.
    pub(in crate::tui) fn rename(&self, title: String) -> anyhow::Result<()> {
        self.ensure_followed()?;
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
        if self.ended() {
            anyhow::bail!(LOST);
        }
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .set_config(params, label)
    }

    /// Queue a change without waiting for the active turn to release it.
    /// Before the session opened, the draft takes it. A setting the daemon
    /// takes counts as chosen, so `/new` carries it on.
    pub(in crate::tui) fn configure(
        &mut self,
        change: rho_protocol::SessionChange,
    ) -> anyhow::Result<()> {
        if self.is_draft() {
            self.configure_draft(change);
            return Ok(());
        }
        if self.ended() {
            anyhow::bail!(LOST);
        }
        self.requests
            .as_ref()
            .expect("attached session has requests")
            .configure(change)
    }
}
