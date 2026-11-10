//! Signing the daemon in and out for the TUI (#1662): the daemon writes the
//! credential store its sessions read, so a key, a login, or a logout made
//! here applies to them from their next prompt.
//!
//! The requests go on the connection the session is attached on, which an
//! interactive login belongs to: the daemon cancels it when that
//! connection closes.

use std::sync::Arc;

use rho_protocol::{
    methods::{AuthKey, AuthLogin, AuthLoginCancel, AuthLogout},
    AuthKeyParams, AuthKeyResponse, AuthLoginCancelParams, AuthLoginParams, AuthLogoutParams,
    ClientEnvironment, LoginFlow, LoginId,
};
use tokio::sync::mpsc;

use super::DaemonSession;
use crate::{app::daemon_client::LoginEvent, tui::daemon_events::LoginUpdate};

/// The next step of the login under way; never resolves without one.
pub(super) async fn next_login(
    logins: &mut Option<mpsc::UnboundedReceiver<LoginEvent>>,
) -> Option<LoginUpdate> {
    let event = match logins {
        Some(logins) => logins.recv().await?,
        None => std::future::pending().await,
    };
    Some(match event {
        LoginEvent::Prompt { login, prompt } => LoginUpdate::Prompt { login, prompt },
        LoginEvent::Finished { login, outcome } => LoginUpdate::Finished { login, outcome },
        LoginEvent::Lost => LoginUpdate::Lost,
    })
}

impl DaemonSession {
    /// Has the daemon check `key` with its provider and save it for `auth`.
    pub(in crate::tui) async fn save_key(
        &self,
        auth: &str,
        key: String,
    ) -> anyhow::Result<AuthKeyResponse> {
        let params = AuthKeyParams {
            auth: auth.to_owned(),
            key,
            environment: ClientEnvironment::of_this_process(),
        };
        Ok(self.daemon.request::<AuthKey>(&params).await?)
    }

    /// Has the daemon delete `auth`'s saved credentials.
    pub(in crate::tui) async fn sign_out(&self, auth: &str) -> anyhow::Result<()> {
        let params = AuthLogoutParams {
            auth: auth.to_owned(),
            environment: ClientEnvironment::of_this_process(),
        };
        self.daemon.request::<AuthLogout>(&params).await?;
        Ok(())
    }

    /// Starts `auth`'s interactive login in the daemon. Its prompt and its
    /// end arrive as `DaemonView::Login`.
    pub(in crate::tui) async fn start_login(
        &mut self,
        auth: &str,
        flow: LoginFlow,
    ) -> anyhow::Result<LoginId> {
        let (sender, logins) = mpsc::unbounded_channel();
        self.daemon.route_login(sender);
        self.logins = Some(logins);
        let params = AuthLoginParams {
            auth: auth.to_owned(),
            flow: Some(flow),
            environment: ClientEnvironment::of_this_process(),
        };
        match self.daemon.request::<AuthLogin>(&params).await {
            Ok(started) => Ok(started.login),
            Err(error) => {
                self.end_login();
                Err(error.into())
            }
        }
    }

    /// Stops listening for the login that ended.
    pub(in crate::tui) fn end_login(&mut self) {
        self.daemon.unroute_login();
        self.logins = None;
    }

    /// Asks the daemon to cancel `login` and stops listening for it. A
    /// login its provider already approved is saved all the same.
    pub(in crate::tui) fn cancel_login(&mut self, login: LoginId) {
        self.end_login();
        let daemon = Arc::clone(&self.daemon);
        tokio::spawn(async move {
            let _ = daemon
                .request::<AuthLoginCancel>(&AuthLoginCancelParams { login })
                .await;
        });
    }
}
