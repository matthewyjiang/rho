use super::{prompt_history_persistence::PromptHistoryOp, App, CommandInvocation, Entry};

impl App {
    pub(super) fn execute_prompt_history_command(
        &mut self,
        invocation: CommandInvocation,
    ) -> anyhow::Result<()> {
        if invocation.args != "clear" {
            self.insert_entry(&Entry::Error("usage: /prompt-history clear".into()));
            self.set_status("invalid prompt-history command");
            return Ok(());
        }

        self.input_ui.clear_history();
        let _ = self.prompt_history_tx.send(PromptHistoryOp::Clear);
        self.set_status("prompt history cleared");
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn take_prompt_history_rx(
        &mut self,
    ) -> Option<
        tokio::sync::mpsc::UnboundedReceiver<super::prompt_history_persistence::PromptHistoryOp>,
    > {
        self.prompt_history_rx.take()
    }
}

#[cfg(test)]
#[path = "prompt_history_command_tests.rs"]
mod tests;
