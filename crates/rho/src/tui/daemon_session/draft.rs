//! A new session before its first prompt, which the daemon does not have
//! yet: the TUI shows the options it will open with, takes setting changes
//! to them as a daemon session would, and keeps the context a `!` command
//! or `/remember` adds. The first prompt opens it with all of that, so a
//! TUI that never prompts leaves nothing behind in the daemon.

use std::sync::Arc;

use anyhow::Context;
use rho_protocol::{methods::SessionNew, ClientEnvironment, SessionChange, SessionNewParams};
use tokio::sync::watch;

use super::{
    attach::{Attached, Connection},
    DaemonSession, Following, Target,
};
use crate::{
    app::{daemon_client::Link, DraftOptions},
    credential_store::AppCredentialStore,
    tui::{
        daemon_events::DaemonView,
        daemon_launch::{DaemonLaunch, InitialSettings},
        daemon_requests::DaemonRequests,
        daemon_settings_read::{load_settings, route_config, settings_loaded},
    },
};

/// A session the first prompt opens.
pub(super) struct Draft {
    pub(super) options: DraftOptions,
    /// Context changes, in order, sent once the session opened and
    /// before its first prompt.
    rows: Vec<SessionChange>,
    /// The connection the settings were read on.
    settings_link: Arc<Link>,
}

impl DaemonSession {
    /// Connects to the daemon, on a connection of its own, for a new
    /// session the first prompt opens with `options`.
    pub(super) async fn drafted(
        launch: DaemonLaunch,
        options: DraftOptions,
    ) -> anyhow::Result<Self> {
        let Connection {
            daemon,
            link,
            config_changes,
            settings,
        } = Connection::open().await?;
        let (target, requests) = watch::channel(Target::Unopened);
        let pending = vec![
            settings_loaded(settings),
            DaemonView::Options(Box::new(options.options())),
        ];
        Ok(Self {
            requests: Some(DaemonRequests::new(requests, Arc::clone(&daemon))),
            daemon,
            launch,
            config_changes,
            target,
            pending,
            logins: None,
            following: Following::Draft(Box::new(Draft {
                options,
                rows: Vec::new(),
                settings_link: link,
            })),
        })
    }

    /// Whether the session waits for its first prompt to open.
    pub(in crate::tui) fn is_draft(&self) -> bool {
        matches!(self.following, Following::Draft(_))
    }

    /// Opens the session the first prompt opens, with the settings
    /// chosen meanwhile, and attaches to it; nothing once it is open. A
    /// setting the daemon refuses fails the open and leaves the draft,
    /// so no turn runs on settings nobody chose; the session it opened
    /// saved nothing, and the daemon discards it unused.
    pub(in crate::tui) async fn open_drafted(&mut self) -> anyhow::Result<()> {
        let Following::Draft(draft) = &self.following else {
            return Ok(());
        };
        let params = SessionNewParams {
            cwd: self.launch.cwd.clone(),
            environment: ClientEnvironment::of_this_process(),
            agent: self.launch.agent.clone(),
            setup: self.launch.setup.clone(),
            parent: None,
            grant: None,
            worktree: false,
        };
        let created = self
            .daemon
            .request::<SessionNew>(&params)
            .await
            .context("could not open a daemon session")?;
        let drafted = InitialSettings::Drafted {
            options: Box::new(draft.options.options()),
            switched_model: draft.options.switched_model(),
        };
        let Attached {
            attachment,
            link,
            views,
            // A new session runs no turn for them to wait for.
            queued: _,
        } = Self::attach(
            &self.daemon,
            created.session,
            Some(params),
            &self.launch,
            drafted,
        )
        .await?;
        // A connection since the settings were read has none of their
        // subscription.
        if !Arc::ptr_eq(&link, &draft.settings_link) {
            self.config_changes = route_config(&self.daemon);
            self.pending
                .push(settings_loaded(load_settings(&link).await));
        }
        // Nothing to replay: the transcript keeps what the TUI showed
        // before the session opened.
        self.pending.extend(
            views
                .into_iter()
                .filter(|view| !matches!(view, DaemonView::Clear)),
        );
        self.target.send_replace(Target::Attached {
            link,
            session: attachment.id.clone(),
        });
        let Following::Draft(draft) = std::mem::replace(
            &mut self.following,
            Following::Attached(Box::new(attachment)),
        ) else {
            unreachable!("the draft was matched above");
        };
        let requests = self
            .requests
            .as_ref()
            .expect("followed session has requests");
        for row in draft.rows {
            requests.carry(row)?;
        }
        Ok(())
    }

    /// Takes `change` for the session not opened yet: a setting applies
    /// to the draft, whose options show as the daemon's would; context
    /// waits for the session to open. Either counts as finished now, as
    /// the app counts every change it asked for.
    pub(super) fn configure_draft(&mut self, change: SessionChange) {
        let Following::Draft(draft) = &mut self.following else {
            return;
        };
        let refused = match change {
            SessionChange::Context { .. } | SessionChange::AddDirectory { .. } => {
                draft.rows.push(change);
                None
            }
            // The session reads both lists as it opens.
            SessionChange::AgentCatalog => None,
            SessionChange::ModelCatalog => {
                draft.options.set_available_auths(
                    rho_providers::credentials::available_auth_modes(&AppCredentialStore),
                );
                self.pending
                    .push(DaemonView::Options(Box::new(draft.options.options())));
                None
            }
            change => match draft.options.apply(&change) {
                Ok(()) => {
                    self.pending
                        .push(DaemonView::Options(Box::new(draft.options.options())));
                    None
                }
                Err(error) => Some(DaemonView::Error(format!(
                    "{}: {error}",
                    crate::tui::daemon_options::change_failure(&change)
                ))),
            },
        };
        self.pending.push(DaemonView::ConfigureFinished);
        self.pending.extend(refused);
    }
}
