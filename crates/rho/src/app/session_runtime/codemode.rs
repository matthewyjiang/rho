//! `codemode.mode` on a running session: which tools the next model request
//! advertises beside `codemode`.
//!
//! Advertisement is resolved per model request, so a change needs no
//! rebuild; the system prompt stays fixed and the model learns of the
//! change from an appended message. Neither mode is a permission level.

use rho_sdk::model::Message;

use super::SessionRuntime;
use crate::config::CodemodeMode;

impl SessionRuntime {
    /// Applies `mode` to later model requests and tells the model. Returns
    /// the notice to show, or `None` when the session already had `mode`.
    /// On error the session keeps its mode.
    pub(crate) fn set_codemode_mode(
        &mut self,
        mode: CodemodeMode,
    ) -> anyhow::Result<Option<String>> {
        let code_mode = self.built.tools.code_mode().clone();
        let previous = code_mode.mode();
        if mode == previous {
            return Ok(None);
        }
        code_mode.set_mode(mode);
        let (context, display) = crate::prompt::codemode_mode_context(mode);
        if let Err(error) = self
            .built
            .session
            .append_message(Message::user_text(context))
        {
            // The model must not see tools it was not told about.
            code_mode.set_mode(previous);
            return Err(error.into());
        }
        Ok(Some(display))
    }
}
