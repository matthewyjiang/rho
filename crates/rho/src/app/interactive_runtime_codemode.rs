//! `/codemode on|off` as a runtime state transition.
//!
//! Like advisor mode, toggling `codemode` changes the advertised tool list, so
//! the runtime rebuilds and rebinds the live session; the system prompt stays
//! fixed and the model learns about the change from an appended notice.
//!
//! The toggle only adds or removes the composition surface. Nested calls always
//! inherit the session permission mode (see `tools::code_mode::nesting`); there
//! is no separate codemode permission level.

use super::InteractiveRuntime;

impl InteractiveRuntime {
    pub(crate) fn codemode_enabled(&self) -> bool {
        self.tools.codemode_registered()
    }

    /// Registers or removes `codemode` for the next turn.
    ///
    /// Returns display text for the transcript notice when the tool list
    /// changed, or `None` when `enabled` already matched. A failed rebuild or
    /// notice leaves the previous tool list and history in place.
    pub(crate) async fn set_codemode(&mut self, enabled: bool) -> anyhow::Result<Option<String>> {
        if enabled == self.tools.codemode_registered() {
            return Ok(None);
        }
        if self.runs.is_active() {
            anyhow::bail!("codemode cannot change while a run is active");
        }
        let history_before = self.sessions.history();
        self.tools.set_codemode_registered(enabled);
        if let Err(error) = self.rebind_current_session().await {
            self.tools.set_codemode_registered(!enabled);
            return Err(error);
        }
        let (model, display) = if enabled {
            let spec = self
                .tools
                .specs()
                .into_iter()
                .find(|spec| spec.name == crate::tools::code_mode::CODEMODE_TOOL_NAME)
                .ok_or_else(|| {
                    anyhow::anyhow!("codemode tool is missing after it was registered")
                })?;
            crate::prompt::codemode_enabled_context(&spec)
        } else {
            crate::prompt::codemode_disabled_context()
        };
        if let Err(error) = self.append_user_context_with_display(model, display.clone()) {
            self.tools.set_codemode_registered(!enabled);
            if self.sessions.history() != history_before {
                let _ = self.sessions.session().replace_history(history_before);
            }
            let _ = self.rebind_current_session().await;
            return Err(error);
        }
        self.remember_tool_list();
        Ok(Some(display))
    }
}
