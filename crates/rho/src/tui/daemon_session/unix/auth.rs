//! Signing the daemon in and out for the TUI (#1662): the daemon writes the
//! credential store its sessions read, so a key, a login, or a logout made
//! here applies to them from their next prompt.
//!
//! The requests go on the connection the session is attached on, never a
//! new one, since an interactive login belongs to its connection: the
//! daemon cancels it when that connection closes.

use std::sync::Arc;

use rho_protocol::{
    methods::{AuthKey, AuthLogin, AuthLoginCancel, AuthLogout},
    AuthKeyParams, AuthKeyResponse, AuthLoginCancelParams, AuthLoginParams, AuthLogoutParams,
    ClientEnvironment, LoginFlow, LoginId,
};
use tokio::sync::mpsc;

use super::{DaemonSession, Target};
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
        let link = self.attached_link().await?;
        Ok(link.request::<AuthKey>(&params).await?)
    }

    /// Has the daemon delete `auth`'s saved credentials.
    pub(in crate::tui) async fn sign_out(&self, auth: &str) -> anyhow::Result<()> {
        let params = AuthLogoutParams {
            auth: auth.to_owned(),
            environment: ClientEnvironment::of_this_process(),
        };
        let link = self.attached_link().await?;
        link.request::<AuthLogout>(&params).await?;
        Ok(())
    }

    /// Starts `auth`'s interactive login in the daemon and waits for its
    /// first step, the prompt unless the login ended before one. The steps
    /// after it arrive as `DaemonView::Login`.
    pub(in crate::tui) async fn start_login(
        &mut self,
        auth: &str,
        flow: LoginFlow,
    ) -> anyhow::Result<(LoginId, LoginUpdate)> {
        let link = self.attached_link().await?;
        let (sender, logins) = mpsc::unbounded_channel();
        self.daemon.route_login(sender);
        self.logins = Some(logins);
        let params = AuthLoginParams {
            auth: auth.to_owned(),
            flow: Some(flow),
            environment: ClientEnvironment::of_this_process(),
        };
        let started = match link.request::<AuthLogin>(&params).await {
            Ok(started) => started.login,
            Err(error) => {
                self.end_login();
                return Err(error.into());
            }
        };
        // The route was dropped only if the connection's router ended.
        let first = next_login(&mut self.logins)
            .await
            .unwrap_or(LoginUpdate::Lost);
        Ok((started, first))
    }

    /// Stops listening for the login that ended.
    pub(in crate::tui) fn end_login(&mut self) {
        self.daemon.unroute_login();
        self.logins = None;
    }

    /// Asks the daemon to cancel `login`. Its end still arrives as a view:
    /// a login its provider approved first is saved all the same.
    pub(in crate::tui) fn cancel_login(&self, login: LoginId) {
        let Target::Attached { link, .. } = &*self.target.borrow() else {
            return;
        };
        let link = Arc::clone(link);
        tokio::spawn(async move {
            let _ = link
                .request::<AuthLoginCancel>(&AuthLoginCancelParams { login })
                .await;
        });
    }
}
