//! A new session before its first prompt, which the daemon does not have
//! yet: the TUI shows the options it will open with, takes setting changes
//! and added directories as a daemon session would, and keeps the context
//! a `!` command or `/remember` adds. The first prompt opens it with all of that, so a
//! TUI that never prompts leaves nothing behind in the daemon.

use std::sync::Arc;

use anyhow::Context;
use rho_protocol::{methods::SessionNew, ClientEnvironment, SessionChange, SessionNewParams};
use tokio::sync::watch;

use super::{
    attach::{Attached, Connection},
    initial_settings::{carry, changes_between, configure_all, ModelChange, Refused},
    DaemonSession, Following, Target, LOST, NOT_OPENED,
};
use crate::{
    app::{daemon_client::Link, DraftOptions, SettingChangeError, SettingOrigin},
    tui::{
        daemon_events::DaemonView,
        daemon_launch::{DaemonLaunch, InitialSettings},
        daemon_options::{change_failure, setting_name},
        daemon_requests::DaemonRequests,
        daemon_settings_read::{load_settings, route_config, settings_loaded},
    },
};

/// A session the first prompt opens.
pub(super) struct Draft {
    pub(super) options: DraftOptions,
    /// Context a `!` command or `/remember` added, in order, sent once
    /// the session opened and before its first prompt.
    context: Vec<SessionChange>,
    /// The connection the settings were read on.
    settings_link: Arc<Link>,
    /// `config.toml` as read when the settings were chosen, without the
    /// launch's flags: a setting it changes since is no longer carried.
    pub(super) chosen_on: Arc<crate::config::Config>,
}

impl DaemonSession {
    /// Connects to the daemon, on a connection of its own, for a new
    /// session the first prompt opens with `options`, whose chosen
    /// settings were chosen on `chosen_on`.
    pub(super) async fn drafted(
        launch: DaemonLaunch,
        options: DraftOptions,
        chosen_on: Arc<crate::config::Config>,
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
                context: Vec::new(),
                settings_link: link,
                chosen_on,
            })),
        })
    }

    /// Whether the session waits for its first prompt to open.
    pub(in crate::tui) fn is_draft(&self) -> bool {
        matches!(self.following, Following::Draft(_))
    }

    /// Opens the session the first prompt opens, with the settings
    /// chosen meanwhile, and attaches to it; nothing once it is open. The
    /// settings apply before the attach; the rest are the config the
    /// daemon read as it opened the session. One carried from the session
    /// `/new` replaced keeps the new session's own when the daemon
    /// refuses it. A chosen one the daemon refuses fails the open and
    /// leaves the draft, so no turn runs on settings nobody chose, and the
    /// session it made, which nothing follows or saved, is discarded at
    /// the daemon's next idle sweeps.
    pub(in crate::tui) async fn open_drafted(&mut self) -> anyhow::Result<()> {
        let Following::Draft(draft) = &self.following else {
            return Ok(());
        };
        let mut setup = self.launch.setup.clone();
        // The launch's, and those `/add-dir` added since.
        setup.additional_directories = draft.options.added_dirs().to_vec();
        let params = SessionNewParams {
            cwd: self.launch.cwd.clone(),
            environment: ClientEnvironment::of_this_process(),
            agent: self.launch.agent.clone(),
            setup,
            parent: None,
            grant: None,
            worktree: false,
        };
        let created = self
            .daemon
            .request::<SessionNew>(&params)
            .await
            .context("could not open a daemon session")?;
        let link = self.daemon.current().await.context(LOST)?;
        let model = if draft.options.switched_model() {
            ModelChange::Switch
        } else {
            ModelChange::Setup
        };
        // Chosen settings must apply; carried ones from the session `/new`
        // replaced apply where the daemon takes them, as they did when
        // `/new` opened the session at once. Both keep the model-first
        // order, since the model decides what the others may pick. The
        // draft's config may be older than the daemon's, whose own stands.
        for change in changes_between(&created.options, &draft.options.options(), model) {
            match draft.options.origins().of(&change) {
                SettingOrigin::Config => {}
                SettingOrigin::Carried => {
                    carry(&link, &created.session, vec![change]).await;
                }
                SettingOrigin::Chosen => {
                    if let Err(refused) = configure_all(&link, &created.session, vec![change]).await
                    {
                        let Refused { change, error } = *refused;
                        return Err(anyhow::Error::new(error).context(format!(
                            "could not set the session's {}",
                            setting_name(&change)
                        )));
                    }
                }
            }
        }
        let origins = draft.options.origins();
        let chosen_on = Arc::clone(&draft.chosen_on);
        let settings_link = Arc::clone(&draft.settings_link);
        let Attached {
            mut attachment,
            link,
            views,
            // A new session runs no turn for them to wait for.
            queued: _,
        } = Self::attach(
            &self.daemon,
            created.session,
            Some(params),
            &self.launch,
            InitialSettings::Kept,
        )
        .await?;
        attachment.origins = origins;
        attachment.chosen_on = chosen_on;
        // A connection since the settings were read has none of their
        // subscription.
        if !Arc::ptr_eq(&link, &settings_link) {
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
        for context in draft.context {
            requests.carry(context)?;
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
        let options = &mut draft.options;
        let applied = match &change {
            SessionChange::Model { model, auth, .. } => options.set_model(model, auth.as_deref()),
            SessionChange::Reasoning { level } => options.set_reasoning(*level),
            SessionChange::FastMode { enabled } => options.set_fast_mode(*enabled),
            SessionChange::PermissionMode { mode } => options.set_permission_mode(*mode),
            SessionChange::Context { .. } => {
                draft.context.push(change.clone());
                Ok(())
            }
            SessionChange::AddDirectory { path } => options.add_directory(path.clone()),
            // The session reads the agent and model lists as it opens; the
            // draft's models follow the auths the app reads
            // (`set_draft_auths`).
            SessionChange::AgentCatalog | SessionChange::ModelCatalog => Ok(()),
            // Each needs a turn or a saved conversation, which a session
            // has only once its first prompt opened it.
            SessionChange::ModelAfterCompaction { .. }
            | SessionChange::SelectNode { .. }
            | SessionChange::RewindWorkspace { .. } => {
                Err(SettingChangeError::Unsupported(NOT_OPENED.into()))
            }
            // `SessionChange` is non-exhaustive; the TUI sends only the above.
            _ => Err(SettingChangeError::Unsupported(NOT_OPENED.into())),
        };
        self.pending.push(DaemonView::ConfigureFinished);
        match (applied, &change) {
            (Ok(()), SessionChange::AddDirectory { path }) => {
                // As the daemon says it of an opened session.
                self.pending.push(DaemonView::Notice(format!(
                    "added directory {}",
                    crate::paths::display(path)
                )));
                self.pending
                    .push(DaemonView::Options(Box::new(draft.options.options())));
            }
            (Ok(()), _) => self
                .pending
                .push(DaemonView::Options(Box::new(draft.options.options()))),
            (Err(error), _) => self.pending.push(DaemonView::Error(format!(
                "{}: {error}",
                change_failure(&change)
            ))),
        }
    }

    /// Lists the models of `auths` in a draft's options, as the app read
    /// them from the credential store; nothing once the session opened,
    /// whose options the daemon reports.
    pub(in crate::tui) fn set_draft_auths(&mut self, auths: &[String]) {
        if let Following::Draft(draft) = &mut self.following {
            if draft.options.set_available_auths(auths) {
                self.pending
                    .push(DaemonView::Options(Box::new(draft.options.options())));
            }
        }
    }
}
