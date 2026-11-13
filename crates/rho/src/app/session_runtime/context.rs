//! Context a host adds to a session between its turns: text the model
//! reads from its next turn on, saved with a row that shows what was added.

use rho_sdk::model::Message;

use super::{InsertedMessage, SessionRuntime};

impl SessionRuntime {
    /// Adds `model` to the model's context and saves it as a user row that
    /// reads `display`, returning that row followed by `following` as
    /// saved. A failed save rolls the context back, so the live session
    /// never holds history its file lacks. No SDK run happens here, so no
    /// turn checkpoint is involved.
    pub(crate) fn append_context<const N: usize>(
        &mut self,
        model: String,
        display: Message,
        following: [Message; N],
    ) -> anyhow::Result<(InsertedMessage, [InsertedMessage; N])> {
        let session = self.built.session.clone();
        let before = session.history();
        session.append_message(Message::user_text(model))?;
        let saved = self.save_prompt_rows(display, following);
        if saved.is_err() {
            session.replace_history(before)?;
        }
        saved
    }
}
