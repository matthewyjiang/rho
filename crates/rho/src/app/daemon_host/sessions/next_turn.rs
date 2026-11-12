//! A loaded session's config (#1720): the one it opened with, which each
//! turn first brings up to date with the global settings that apply at a
//! loaded session's next turn: the advisor, automatic compaction, and the
//! codemode mode, with the configured providers and the title and
//! compaction models that resolve on them. A permission-mode change takes
//! the permission classifier's models the same way.
//!
//! Lazy on purpose: an idle session pays nothing for a change, and the turn
//! already holds the session's runtime to apply it.

use std::sync::{Arc, Mutex, PoisonError};

use super::LoadedSession;
use crate::{
    agent::PERMISSION_CLASSIFIER_AGENT_ID,
    app::session_runtime::SessionRuntime,
    config::Config,
    permission_classifier::DECISION_SCREEN_ID,
    tools::advisor::{advisor_available, advisor_model},
};

/// What a loaded session's turns read.
pub(super) struct SessionConfig {
    config: Arc<Config>,
    /// The global config the session last took its next-turn settings
    /// from, as the settings store published it.
    applied: Arc<Config>,
}

impl SessionConfig {
    /// `config`, opened from the global config `applied`.
    pub(super) fn new(config: Config, applied: Arc<Config>) -> Mutex<Self> {
        Mutex::new(Self {
            config: Arc::new(config),
            applied,
        })
    }
}

impl LoadedSession {
    /// The config the session's turns read now.
    pub(super) fn config(&self) -> Arc<Config> {
        Arc::clone(&self.lock_config().config)
    }

    /// The config the turn about to start on `runtime` reads: the session's,
    /// with the next-turn settings of the global config as it is now,
    /// applied to `runtime` when they changed since the session's last
    /// turn. Tells the session's clients what the model was told, and what
    /// could not be applied, which leaves the session as it was until a
    /// later turn applies it.
    pub(super) async fn config_for_turn(&self, runtime: &mut SessionRuntime) -> Arc<Config> {
        let (current, applied) = {
            let config = self.lock_config();
            (Arc::clone(&config.config), Arc::clone(&config.applied))
        };
        // No config loaded yet: the session keeps the one it opened with.
        let Ok(global) = self.settings.agent_config().await else {
            return current;
        };
        if Arc::ptr_eq(&global, &applied) {
            return current;
        }
        let mut next = (*current).clone();
        // Only what changed globally: a re-read that changed nothing, or
        // changed other settings, leaves the agent's own values alone.
        let changes = next.take_next_turn_settings(&applied, &global);
        let mut notices = Vec::new();
        let mut failed = false;
        // Compaction first: the advisor's rebuild reads `next`, which must
        // hold only compaction settings the session took.
        if changes.compaction {
            if let Err(error) = runtime.set_compaction(&next) {
                failed = true;
                notices.push(format!("could not apply the compaction settings: {error}"));
                next.take_compaction_settings(&current);
            }
        }
        if changes.advisor {
            let model = advisor_available(&next)
                .then(|| advisor_model(&next).cloned())
                .flatten();
            match runtime.set_advisor(model, &next).await {
                Ok(notice) => notices.extend(notice),
                Err(error) => {
                    failed = true;
                    notices.push(format!("could not apply the advisor settings: {error}"));
                    next.take_advisor_settings(&current);
                }
            }
        }
        if changes.codemode {
            match runtime.set_codemode_mode(next.codemode.mode) {
                Ok(notice) => notices.extend(notice),
                Err(error) => {
                    failed = true;
                    notices.push(format!("could not apply the codemode mode: {error:#}"));
                    next.codemode.mode = current.codemode.mode;
                }
            }
        }
        if !notices.is_empty() {
            self.feed.publish(
                notices
                    .into_iter()
                    .map(|text| rho_protocol::SessionEvent::Notice { text }),
            );
        }
        let next = Arc::new(next);
        *self.lock_config() = SessionConfig {
            config: Arc::clone(&next),
            // A change that failed is tried again at the next turn; the
            // flags only name what still differs from `global`.
            applied: if failed { applied } else { global },
        };
        next
    }

    /// The config a permission-mode change reads: the session's, with the
    /// permission classifier's models of the global config as it is now. A
    /// classifier picked after the session opened then runs when a client
    /// sets Auto; [`Self::keep_config`] keeps it once the mode applied.
    pub(super) async fn config_for_permission_change(&self) -> Arc<Config> {
        let Ok(global) = self.settings.agent_config().await else {
            return self.config();
        };
        let mut next = (*self.config()).clone();
        for id in [PERMISSION_CLASSIFIER_AGENT_ID, DECISION_SCREEN_ID] {
            next.take_internal_agent(&global, id);
        }
        Arc::new(next)
    }

    /// Makes `config` the one the session's turns and rebuilds read.
    pub(super) fn keep_config(&self, config: Arc<Config>) {
        self.lock_config().config = config;
    }

    /// Nothing panics while holding the lock, so a poisoned one is still
    /// consistent.
    fn lock_config(&self) -> std::sync::MutexGuard<'_, SessionConfig> {
        self.config.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
