//! Selection identity and action policy for an open picker.
//!
//! Feature modules construct a picker through [`super::UiPicker`] named
//! constructors. Matching on [`PickerAction`] stays in `crate::tui::picker_actions`.

/// What confirming the highlighted row should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum PickerAction {
    SelectModel,
    SelectInternalAgentModel,
    LoginGroup,
    LoginProvider,
    LogoutProvider,
    SwitchAuthMode,
    RefreshModelList,
    InsertSkillCommand,
    ViewAgent,
    /// Read-only MCP server inventory. Distinct from `Dismiss` so background
    /// refreshes can tell this picker apart without reading its title.
    ViewMcpServers,
    ResumeSession,
    ManageSessions,
    SelectTreeNode,
    SelectRewindCheckpoint,
    ConfirmRewindCheckpoint,
    Config,
    SelectTheme,
    EditAgent,
    Workflow,
    AttachSubagent,
    Dismiss,
}

/// Whether a picker commit runs idle or during a model turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum PickerTurn {
    Idle,
    DuringTurn,
}

/// Config row to re-select after a child picker closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum ConfigParentRow {
    ConversationModel,
    Theme,
    LogoutProvider,
    SwitchAuthMode,
    RefreshModelList,
}

/// What confirming a row does while a model turn is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum DuringTurnSelect {
    Apply,
    CloseOnly,
    Unavailable(&'static str),
}

impl PickerAction {
    pub(in crate::tui) fn space_confirms_selection(self) -> bool {
        matches!(self, PickerAction::Config)
    }

    /// Whether the filter uses regex matching instead of fuzzy ranking.
    pub(in crate::tui) fn uses_regex_filter(self) -> bool {
        !matches!(
            self,
            PickerAction::SelectModel
                | PickerAction::SelectInternalAgentModel
                | PickerAction::SelectTheme
        )
    }

    pub(in crate::tui) fn default_confirm_verb(self) -> &'static str {
        match self {
            PickerAction::Config => "change",
            PickerAction::Dismiss | PickerAction::ViewMcpServers | PickerAction::ViewAgent => {
                "close"
            }
            PickerAction::RefreshModelList => "refresh",
            PickerAction::SelectModel
            | PickerAction::SelectInternalAgentModel
            | PickerAction::SelectTheme
            | PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::SwitchAuthMode
            | PickerAction::InsertSkillCommand
            | PickerAction::ResumeSession
            | PickerAction::ManageSessions
            | PickerAction::SelectTreeNode
            | PickerAction::SelectRewindCheckpoint
            | PickerAction::ConfirmRewindCheckpoint
            | PickerAction::EditAgent
            | PickerAction::Workflow
            | PickerAction::AttachSubagent => "select",
        }
    }

    pub(in crate::tui) fn is_model_list(self) -> bool {
        matches!(
            self,
            PickerAction::SelectModel | PickerAction::SelectInternalAgentModel
        )
    }

    pub(in crate::tui) fn keeps_composer_open(self, turn: PickerTurn) -> bool {
        match turn {
            PickerTurn::Idle => matches!(
                self,
                PickerAction::Config
                    | PickerAction::LoginGroup
                    | PickerAction::ViewAgent
                    | PickerAction::EditAgent
                    | PickerAction::SelectRewindCheckpoint
                    | PickerAction::ConfirmRewindCheckpoint
                    | PickerAction::Workflow
                    | PickerAction::ManageSessions
            ),
            PickerTurn::DuringTurn => matches!(self, PickerAction::Config),
        }
    }

    pub(in crate::tui) fn during_turn_select(self) -> DuringTurnSelect {
        match self {
            PickerAction::InsertSkillCommand
            | PickerAction::AttachSubagent
            | PickerAction::Config
            | PickerAction::SelectModel
            | PickerAction::SelectTheme => DuringTurnSelect::Apply,
            PickerAction::Dismiss | PickerAction::ViewMcpServers | PickerAction::ViewAgent => {
                DuringTurnSelect::CloseOnly
            }
            PickerAction::SelectInternalAgentModel | PickerAction::EditAgent => {
                DuringTurnSelect::Unavailable(
                    "agent editing is unavailable while a model turn is running",
                )
            }
            PickerAction::ResumeSession
            | PickerAction::ManageSessions
            | PickerAction::SelectTreeNode
            | PickerAction::SelectRewindCheckpoint
            | PickerAction::ConfirmRewindCheckpoint
            | PickerAction::Workflow => DuringTurnSelect::Unavailable(
                "workflow and session navigation are unavailable while a model turn is running",
            ),
            PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::LogoutProvider
            | PickerAction::SwitchAuthMode
            | PickerAction::RefreshModelList => DuringTurnSelect::Unavailable(
                "that picker action is unavailable while a model turn is running",
            ),
        }
    }

    pub(in crate::tui) fn config_parent_row(self) -> Option<ConfigParentRow> {
        match self {
            PickerAction::SelectModel => Some(ConfigParentRow::ConversationModel),
            PickerAction::SelectTheme => Some(ConfigParentRow::Theme),
            PickerAction::LogoutProvider => Some(ConfigParentRow::LogoutProvider),
            PickerAction::SwitchAuthMode => Some(ConfigParentRow::SwitchAuthMode),
            PickerAction::RefreshModelList => Some(ConfigParentRow::RefreshModelList),
            PickerAction::SelectInternalAgentModel
            | PickerAction::LoginGroup
            | PickerAction::LoginProvider
            | PickerAction::InsertSkillCommand
            | PickerAction::ViewAgent
            | PickerAction::ViewMcpServers
            | PickerAction::ResumeSession
            | PickerAction::ManageSessions
            | PickerAction::SelectTreeNode
            | PickerAction::SelectRewindCheckpoint
            | PickerAction::ConfirmRewindCheckpoint
            | PickerAction::Config
            | PickerAction::EditAgent
            | PickerAction::Workflow
            | PickerAction::AttachSubagent
            | PickerAction::Dismiss => None,
        }
    }
}
