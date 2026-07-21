#[derive(Clone, Debug, PartialEq)]
pub(super) struct TurnPrompt {
    pub(super) model: String,
    pub(super) display: String,
    pub(super) history: String,
    pub(super) persisted_display: Option<String>,
    pub(super) initial_tool_call: Option<rho_sdk::model::ToolCall>,
}

impl TurnPrompt {
    pub(super) fn standard(model: String, display: String) -> Self {
        Self {
            history: model.clone(),
            model,
            display,
            persisted_display: None,
            initial_tool_call: None,
        }
    }

    pub(super) fn command(model: String, command: String) -> Self {
        Self {
            model,
            display: command.clone(),
            history: command.clone(),
            persisted_display: Some(command),
            initial_tool_call: None,
        }
    }

    pub(super) fn with_initial_tool_call(mut self, call: rho_sdk::model::ToolCall) -> Self {
        self.initial_tool_call = Some(call);
        self
    }
}
