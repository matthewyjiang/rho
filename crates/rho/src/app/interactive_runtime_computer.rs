//! Live desktop authority and model-visible capability updates.

use crate::{
    permission::PermissionMode,
    tools::computer_use::{ComputerUseSession, ComputerUseStatus},
};

use super::InteractiveRuntime;

const CONTEXT_PREFIX: &str = "[computer use context]\n";

#[cfg(test)]
#[path = "interactive_runtime_computer_tests.rs"]
mod tests;

pub(crate) enum ComputerUseUpdate {
    Unchanged,
    Connected,
    ConnectionFailed(String),
    Revoked(String),
}

pub(super) enum ComputerPreferenceSource {
    NewSession,
    SavedSession,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ComputerUseEligibilityError {
    #[error("computer use can only be enabled while the session is idle")]
    Busy,
    #[error("computer use is unavailable in plan mode")]
    PlanMode,
    #[error("computer use requires an interactive native session with tools enabled")]
    UnsupportedHost,
}

impl InteractiveRuntime {
    pub(crate) fn computer_use(&self) -> Option<&ComputerUseSession> {
        self.tools.computer_use()
    }

    pub(crate) fn computer_use_eligibility(
        &self,
    ) -> Result<&ComputerUseSession, ComputerUseEligibilityError> {
        if self.is_session_busy() {
            return Err(ComputerUseEligibilityError::Busy);
        }
        if self.permission_mode == PermissionMode::Plan {
            return Err(ComputerUseEligibilityError::PlanMode);
        }
        self.tools
            .computer_use()
            .ok_or(ComputerUseEligibilityError::UnsupportedHost)
    }

    pub(crate) fn enable_computer_use(&self) -> anyhow::Result<()> {
        self.computer_use_eligibility()?.start_connect()
    }

    pub(crate) fn install_computer_driver(&self) -> anyhow::Result<std::path::PathBuf> {
        self.computer_use_eligibility()?.start_installation()
    }

    /// The idle boundary owns registration for both activation and revocation,
    /// including revocations from retained tools or the during-turn UI handle.
    /// Driver failures are updates, not runtime failures. Only registration or
    /// session rebind errors may prevent the next turn from starting.
    pub(crate) async fn reconcile_computer_use(&mut self) -> anyhow::Result<ComputerUseUpdate> {
        if self.is_session_busy() {
            return Ok(ComputerUseUpdate::Unchanged);
        }
        let Some(session) = self.computer_use().cloned() else {
            return Ok(ComputerUseUpdate::Unchanged);
        };
        let connection_result = session.take_connect_result().await;
        session.finish_closing(/*wait*/ false).await;
        let desired = session.status() == ComputerUseStatus::Connected;
        let registration_changed = self.tools.set_computer_use_registered(desired);
        self.computer_runtime_dirty |= registration_changed;
        if registration_changed {
            self.remember_tool_list();
        }
        if self.computer_runtime_dirty && self.sessions.pending_replacement().is_none() {
            if let Err(error) = self.rebind_current_session().await {
                // The dirty flag retries the bind at the next idle boundary.
                // Retained tools fail closed immediately, even if rebinding fails.
                session.revoke();
                return Err(error);
            }
        }
        if (registration_changed || self.computer_context.is_none())
            && self.sessions.pending_replacement().is_none()
        {
            if let Err(error) = self.refresh_computer_context() {
                self.computer_context = None;
                return Err(error);
            }
        }
        Ok(match connection_result {
            Some(Ok(())) if desired => ComputerUseUpdate::Connected,
            Some(Err(error)) => ComputerUseUpdate::ConnectionFailed(error.to_string()),
            Some(Ok(())) | None => session
                .take_revocation_notice()
                .map_or(ComputerUseUpdate::Unchanged, ComputerUseUpdate::Revoked),
        })
    }

    pub(crate) async fn disable_computer_use(&mut self) -> anyhow::Result<()> {
        self.revoke_computer_use();
        if let ComputerUseUpdate::Revoked(notice) = self.reconcile_computer_use().await? {
            self.sessions.queue_notice(notice);
        }
        Ok(())
    }

    /// Revoke authority and unregister without rebuilding a runtime the caller
    /// will replace. Failed replacements leave a dirty bind for the next boundary.
    pub(super) fn revoke_computer_use(&mut self) {
        if let Some(session) = self.tools.computer_use() {
            session.revoke();
        }
        self.computer_runtime_dirty |= self.tools.set_computer_use_registered(false);
        self.remember_tool_list();
        // Lifecycle callers must not write a disabled notice into the session
        // they are leaving. A retained session gets fresh context before its next call.
        self.computer_context = None;
    }

    /// A new conversation gets a fresh grant only from machine-local consent.
    pub(super) async fn restore_computer_preference(&mut self, source: ComputerPreferenceSource) {
        let Some(session) = self.computer_use().cloned() else {
            return;
        };
        session.finish_closing(/*wait*/ true).await;
        use crate::tools::computer_use::ComputerUsePreference;
        let preference = match source {
            ComputerPreferenceSource::NewSession => {
                ComputerUsePreference::initialize_session(self.session_id())
            }
            ComputerPreferenceSource::SavedSession => {
                ComputerUsePreference::load_session(self.session_id())
            }
        };
        let result = match preference {
            Ok(ComputerUsePreference::Enabled) if self.permission_mode != PermissionMode::Plan => {
                session.start_connect()
            }
            Ok(ComputerUsePreference::Enabled | ComputerUsePreference::Disabled) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            self.sessions.queue_notice(format!(
                "could not restore computer use preference: {error}; desktop access remains off"
            ));
        }
    }

    /// Derive context from live authority, never from the saved preference.
    /// The acknowledgement resets on session replacement and compaction, so
    /// resumed history never determines whether desktop authority is available.
    pub(crate) fn pending_computer_context(&self) -> Option<(String, String)> {
        // Unsupported hosts need no desktop notice unless resumed history
        // contains one to supersede, for example when resuming with --no-tools.
        if self.computer_use().is_none() && self.computer_context.is_none()
            && !self.sessions.history().iter().any(|message| {
                matches!(message, rho_sdk::model::Message::User(blocks) if blocks.iter().any(|block|
                    matches!(block, rho_sdk::model::ContentBlock::Text(text) if text.contains(CONTEXT_PREFIX))))
            })
        {
            return None;
        }
        let enabled = self
            .computer_use()
            .is_some_and(|session| session.status() == ComputerUseStatus::Connected);
        let state = if enabled { "enabled" } else { "disabled" };
        let mut context = format!("{CONTEXT_PREFIX}{state}\nThis is a runtime capability update, not a user request. It supersedes earlier computer-use state.\n");
        if enabled {
            context.push_str("The computer tool is available. First call computer with {\"action\":\"list\"} to discover desktop capabilities and instructions. Desktop access includes signed-in apps; follow the user's task and do not treat access as blanket authorization.\n");
            if let Some(spec) = self
                .tools
                .specs()
                .into_iter()
                .find(|spec| spec.name == "computer")
            {
                context.push_str(&crate::prompt::tool_schema_block(&spec));
            }
        } else {
            context.push_str("Desktop access is off. Do not attempt computer tool calls. Only the user can enable access with /computer on; do not seek another route around disabled desktop access.\n");
        }
        if self.computer_context.as_deref() == Some(context.as_str()) {
            return None;
        }
        Some((context, format!("computer use {state}")))
    }

    pub(super) fn refresh_computer_context(&mut self) -> anyhow::Result<()> {
        if let Some((model, display)) = self.pending_computer_context() {
            self.append_user_context_with_display(model.clone(), display)?;
            self.acknowledge_computer_context(model);
        }
        Ok(())
    }

    pub(crate) fn acknowledge_computer_context(&mut self, context: String) {
        self.computer_context = Some(context);
    }
}
