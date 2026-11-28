//! Context a host adds to a session between its turns: text the model
//! reads from its next turn on, saved with a row that shows what was added.

use rho_sdk::model::Message;

use super::{InsertedMessage, SessionRuntime};
use crate::session::tree::MessageKey;

impl SessionRuntime {
    /// Adds `model` to the model's context and saves it as a user row that
    /// reads `display`, under `display_key` when given (see
    /// [`Self::save_prompt_rows`]), returning that row followed by
    /// `following` as saved. A failed save rolls the context back, so the
    /// live session never holds history its file lacks. No SDK run happens
    /// here, so no turn checkpoint is involved.
    pub(crate) fn append_context<const N: usize>(
        &mut self,
        model: String,
        display: Message,
        display_key: Option<MessageKey>,
        following: [Message; N],
    ) -> anyhow::Result<(InsertedMessage, [InsertedMessage; N])> {
        let session = self.built.session.clone();
        let before = session.history();
        session.append_message(Message::user_text(model))?;
        let saved = self.save_prompt_rows(display, display_key, following);
        if saved.is_err() {
            session.replace_history(before)?;
        }
        saved
    }
}
