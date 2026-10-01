//! `/codemode on|only` (Pi's `codemode.mode`) as a runtime state transition.
//!
//! `codemode` stays registered in both modes. The mode only changes which
//! tools the SDK advertises, and advertisement is resolved per model request
//! through [`crate::tools::AppToolSet::tool_visibility`], so no runtime rebuild
//! is needed. The system prompt stays fixed; the model learns about the change
//! from an appended notice.
//!
//! Neither mode is a permission level. Nested calls always inherit the session
//! permission mode (see `tools::code_mode::nesting`).

use crate::config::CodemodeMode;

use super::InteractiveRuntime;

impl InteractiveRuntime {
    pub(crate) fn codemode_mode(&self) -> CodemodeMode {
        self.tools.codemode_mode()
    }

    /// Applies `mode` to the next model request.
    ///
    /// Returns display text for the transcript notice when the mode changed, or
    /// `None` when it already matched. A failed notice restores the previous
    /// mode and history.
    pub(crate) fn set_codemode_mode(
        &mut self,
        mode: CodemodeMode,
    ) -> anyhow::Result<Option<String>> {
        let previous = self.tools.codemode_mode();
        if mode == previous {
            return Ok(None);
        }
        if self.runs.is_active() {
            anyhow::bail!("codemode mode cannot change while a run is active");
        }
        self.tools.set_codemode_mode(mode);
        let (model, display) = crate::prompt::codemode_mode_context(mode);
        if let Err(error) = self.append_user_context_with_display(model, display.clone()) {
            self.tools.set_codemode_mode(previous);
            return Err(error);
        }
        self.remember_tool_list();
        Ok(Some(display))
    }
}
