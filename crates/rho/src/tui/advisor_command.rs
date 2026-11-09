//! The advisor outside `/advisor`: the command's usage, and applying an
//! advisor model `/agents` saved to the in-process runtime. `/advisor`
//! itself changes the daemon's settings (`settings_screen`).

use crate::agent::ADVISOR_AGENT_ID;

use super::{App, Entry, InteractiveRuntime};

pub(super) const ADVISOR_USAGE: &str = "usage: /advisor [on|off|model]";

impl App {
    /// Applies the saved advisor state to the live runtime.
    ///
    /// The advisor model and the mode reach the runtime as one value, because
    /// advisor mode without a model offers the executor nothing.
    pub(super) async fn sync_advisor_runtime(&mut self, agent: &mut InteractiveRuntime) {
        let model = self.info.runtime.advisor_mode.then(|| {
            self.info
                .runtime
                .internal_agents
                .get(ADVISOR_AGENT_ID)
                .cloned()
        });
        self.info
            .services
            .diagnostics
            .update_advisor_mode(self.info.runtime.advisor_mode);
        match agent.set_advisor(model.flatten()).await {
            Ok(Some(display)) => {
                self.insert_entry(&Entry::Notice(display));
                self.info
                    .services
                    .diagnostics
                    .update_tools(&agent.tool_specs());
            }
            Ok(None) => {}
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "advisor mode could not be applied to this session: {error}"
                )));
            }
        }
    }
}
