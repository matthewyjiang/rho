//! A loaded session's config (#1720): the one it opened with, which each
//! turn first brings up to date with the global settings that apply at a
//! loaded session's next turn, the advisor and automatic compaction.
//!
//! Lazy on purpose: an idle session pays nothing for a change, and the turn
//! already holds the session's runtime to apply it.

use std::sync::{Arc, Mutex, PoisonError};

use super::LoadedSession;
use crate::{
    app::session_runtime::SessionRuntime,
    config::Config,
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
    /// could not be applied, which leaves the session as it was.
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
        let changes = next.take_next_turn_settings(&global);
        let mut notices = Vec::new();
        if changes.advisor {
            let model = advisor_available(&next)
                .then(|| advisor_model(&next).cloned())
                .flatten();
            match runtime.set_advisor(model, &next).await {
                Ok(notice) => notices.extend(notice),
                Err(error) => {
                    notices.push(format!("could not apply the advisor settings: {error}"));
                    next.advisor_mode = current.advisor_mode;
                    restore_advisor_model(&mut next, &current);
                }
            }
        }
        if changes.compaction {
            if let Err(error) = runtime.set_compaction(&next) {
                notices.push(format!("could not apply the compaction settings: {error}"));
                next.auto_compact = current.auto_compact;
                next.compact_threshold_percent = current.compact_threshold_percent;
                next.compact_target_percent = current.compact_target_percent;
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
            applied: global,
        };
        next
    }

    /// Nothing panics while holding the lock, so a poisoned one is still
    /// consistent.
    fn lock_config(&self) -> std::sync::MutexGuard<'_, SessionConfig> {
        self.config.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Gives `config` the advisor model `from` has.
fn restore_advisor_model(config: &mut Config, from: &Config) {
    use crate::agent::ADVISOR_AGENT_ID;
    match from.internal_agents.get(ADVISOR_AGENT_ID) {
        Some(selection) => {
            config
                .internal_agents
                .insert(ADVISOR_AGENT_ID.into(), selection.clone());
        }
        None => {
            config.internal_agents.remove(ADVISOR_AGENT_ID);
        }
    }
}
