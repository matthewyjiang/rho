//! Starts interactive runs after rebuilding pending session replacements.
use super::*;

enum TurnPrelude {
    None,
    ToolCall(ToolCall),
}

impl InteractiveRuntime {
    #[cfg(test)]
    pub(crate) async fn start(
        &mut self,
        input: UserInput,
        display_user: Option<Message>,
    ) -> Result<(), Error> {
        self.start_run(
            input,
            display_user,
            TurnPrelude::None,
            /*boundary_inputs*/ None,
        )
        .await
    }

    pub(crate) async fn start_with_boundary_inputs(
        &mut self,
        input: UserInput,
        display_user: Option<Message>,
        tool_call: Option<ToolCall>,
    ) -> Result<tokio::sync::mpsc::Receiver<rho_sdk::BoundaryInputRequest>, Error> {
        let (source, receiver) = rho_sdk::boundary_input_channel();
        let prelude = tool_call.map_or(TurnPrelude::None, TurnPrelude::ToolCall);
        self.start_run(input, display_user, prelude, Some(source))
            .await?;
        Ok(receiver)
    }

    async fn start_run(
        &mut self,
        input: UserInput,
        display_user: Option<Message>,
        prelude: TurnPrelude,
        boundary_inputs: Option<rho_sdk::BoundaryInputSource>,
    ) -> Result<(), Error> {
        if self.runs.state() != InteractiveState::Idle || self.is_compacting() {
            return Err(Error::SessionBusy);
        }
        if let Some(source) = self.sessions.pending_replacement() {
            self.rebuild_session(
                source,
                ReplacementLifecycle::Started,
                SessionWriteRetention::Keep,
            )
            .await
            .map_err(|error| Error::Persistence {
                message: error.to_string(),
            })?;
        }
        self.sessions
            .session()
            .set_boundary_inputs(boundary_inputs)?;
        let model_user = Message::User(input.blocks().to_vec());
        let mut request_history = self.sessions.history();
        let pending_turn = PendingTurn::new(model_user, display_user, request_history.len());
        request_history.push(Message::User(input.blocks().to_vec()));
        let context_usage = rho_sdk::model::ContextUsage::estimated(
            rho_sdk::model::context::estimate_context_tokens(&request_history, &self.tools.specs()),
            self.context_window,
        );
        self.tools
            .checkpoint_tracker()
            .begin_turn(self.sessions.storage())
            .map_err(|error| Error::Persistence {
                message: error.to_string(),
            })?;
        let run_result = match prelude {
            TurnPrelude::None => self.sessions.session().start(input).await,
            TurnPrelude::ToolCall(call) => {
                self.sessions
                    .session()
                    .start_with_tool_call(input, call)
                    .await
            }
        };
        let run = match run_result {
            Ok(run) => run,
            Err(error) => {
                self.tools.checkpoint_tracker().discard_turn();
                return Err(error);
            }
        };
        if let Err(error) = self.runs.begin(run, pending_turn, context_usage) {
            self.tools.checkpoint_tracker().discard_turn();
            return Err(error);
        }
        Ok(())
    }
}
