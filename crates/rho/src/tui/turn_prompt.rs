#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TurnPrompt {
    pub(super) model: String,
    pub(super) display: String,
    pub(super) history: String,
    pub(super) persisted_display: Option<String>,
}

impl TurnPrompt {
    pub(super) fn standard(model: String, display: String) -> Self {
        Self {
            history: model.clone(),
            model,
            display,
            persisted_display: None,
        }
    }

    pub(super) fn command(model: String, command: String) -> Self {
        Self {
            model,
            display: command.clone(),
            history: command.clone(),
            persisted_display: Some(command),
        }
    }
}
