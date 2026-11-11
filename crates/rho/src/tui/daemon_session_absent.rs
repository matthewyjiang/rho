//! The daemon session where there is no daemon (#1665): uninhabited, so the
//! TUI's daemon paths compile and never run.

use super::{super::daemon_launch::DaemonTarget, DaemonView};

pub(crate) enum DaemonSession {}

impl DaemonSession {
    pub(in crate::tui) fn configure(
        &self,
        _change: rho_protocol::SessionChange,
    ) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) fn request_mcp(&self) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) fn tool_sources_loading(&self) -> bool {
        match *self {}
    }

    pub(in crate::tui) fn tool_sources_loaded(&mut self) {
        match *self {}
    }

    pub(in crate::tui) fn goal(
        &self,
        _action: rho_protocol::SessionGoalAction,
    ) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) fn rename(&self, _title: String) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) fn deleter(&self) -> crate::tui::sessions_hub_tasks::DeleteHeld {
        match *self {}
    }

    pub(in crate::tui) fn inspect(
        &self,
        _report: rho_protocol::SessionReport,
    ) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) async fn provider_context(
        &self,
    ) -> anyhow::Result<crate::app::provider_context_report::ProviderContextReport> {
        match *self {}
    }

    pub(in crate::tui) async fn save_key(
        &self,
        _auth: &str,
        _key: String,
    ) -> anyhow::Result<rho_protocol::AuthKeyResponse> {
        match *self {}
    }

    pub(in crate::tui) async fn sign_out(&self, _auth: &str) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) async fn start_login(
        &mut self,
        _auth: &str,
        _flow: rho_protocol::LoginFlow,
    ) -> anyhow::Result<(
        rho_protocol::LoginId,
        crate::tui::daemon_events::LoginUpdate,
    )> {
        match *self {}
    }

    pub(in crate::tui) fn end_login(&mut self) {
        match *self {}
    }

    pub(in crate::tui) fn cancel_login(&self, _login: rho_protocol::LoginId) {
        match *self {}
    }

    pub(in crate::tui) fn request_jobs(&self) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) fn set_config(
        &self,
        _params: rho_protocol::ConfigSetParams,
        _label: String,
    ) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) fn id(&self) -> &str {
        match *self {}
    }

    pub(in crate::tui) async fn open_sibling(&self, _target: DaemonTarget) -> anyhow::Result<Self> {
        match *self {}
    }

    pub(crate) fn saved_id(&self) -> Option<&str> {
        match *self {}
    }

    pub(crate) fn workspace(&self) -> &std::path::Path {
        match *self {}
    }

    pub(super) async fn next_views(&mut self) -> Vec<DaemonView> {
        match *self {}
    }

    pub(in crate::tui) fn take_views(&mut self) -> Vec<DaemonView> {
        match *self {}
    }

    pub(in crate::tui) async fn prompt(
        &mut self,
        _content: Vec<rho_protocol::ContentBlock>,
    ) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) async fn cancel(&self) -> anyhow::Result<()> {
        match *self {}
    }

    pub(in crate::tui) async fn steer(&self, _text: String) -> anyhow::Result<rho_sdk::SteeringId> {
        match *self {}
    }

    pub(in crate::tui) async fn unsteer(
        &self,
        _id: &rho_sdk::SteeringId,
    ) -> anyhow::Result<rho_protocol::UnsteerOutcome> {
        match *self {}
    }

    pub(in crate::tui) async fn answer(
        &mut self,
        _id: rho_protocol::InputRequestId,
        _answer: rho_protocol::InputAnswer,
    ) {
        match *self {}
    }

    pub(in crate::tui) async fn hold(&mut self, _id: rho_protocol::InputRequestId) -> bool {
        match *self {}
    }
}
