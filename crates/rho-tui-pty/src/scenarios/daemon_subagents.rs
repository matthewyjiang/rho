//! The subagent, attach-view and tool-turn scenarios again, with the session
//! in the daemon and the TUI following it (`RHO_TUI_DAEMON=1`, #1667). Each
//! runs the steps of the in-process scenario it names, so the two cannot
//! drift.

use super::{
    agent_messages::AGENT_MESSAGES_SCENARIO,
    agent_prompt::AGENT_PROMPT_SCENARIO,
    attach_picker::ATTACH_PICKER_SCENARIO,
    attach_view::{
        ATTACH_PARENT_MESSAGE_SCENARIO, ATTACH_VIEW_CYCLE_SCENARIO,
        ATTACH_VIEW_FROM_COMMAND_SCENARIO, ATTACH_VIEW_PARENT_APPROVAL_SCENARIO,
        ATTACH_VIEW_QUIT_RESTORES_SCENARIO,
    },
    background_agents::{
        BACKGROUND_AGENT_AUTO_DELIVERY_SCENARIO, BACKGROUND_AGENT_QUESTIONNAIRE_SCENARIO,
        CANCEL_BACKGROUND_AGENT_QUESTIONNAIRE_SCENARIO,
        LATE_BACKGROUND_AGENT_QUESTIONNAIRE_SCENARIO,
    },
    quiet_subagent::{
        COMPLETION_AFTER_NEW_SCENARIO, QUIET_SUBAGENT_SCENARIO, RUNNING_NOTICES_SCENARIO,
        STREAMING_NOTICE_SCENARIO,
    },
    steering::RETRACT_STEERING_DURING_TOOL_SCENARIO,
    subagent_rail::SUBAGENT_RAIL_MOUSE_SCENARIO,
};
use crate::scenario::Scenario;

const DAEMON_SESSION: &[(&str, &str)] = &[("RHO_TUI_DAEMON", "1")];

/// `base` under another id, with its session in the daemon.
const fn in_daemon(id: &'static str, base: &Scenario) -> Scenario {
    Scenario {
        id,
        description: base.description,
        size: base.size,
        setup: base.setup,
        env: DAEMON_SESSION,
        args: base.args,
        steps: base.steps,
        smoke: false,
    }
}

pub(super) const AGENT_PROMPT_STREAMING: Scenario =
    in_daemon("daemon_agent_prompt_streaming", &AGENT_PROMPT_SCENARIO);

pub(super) const AGENT_MESSAGES: Scenario =
    in_daemon("daemon_agent_messages", &AGENT_MESSAGES_SCENARIO);

pub(super) const BACKGROUND_AGENT_AUTO_DELIVERY: Scenario = in_daemon(
    "daemon_background_agent_auto_delivery",
    &BACKGROUND_AGENT_AUTO_DELIVERY_SCENARIO,
);

pub(super) const BACKGROUND_AGENT_QUESTIONNAIRE: Scenario = in_daemon(
    "daemon_background_agent_questionnaire",
    &BACKGROUND_AGENT_QUESTIONNAIRE_SCENARIO,
);

pub(super) const LATE_BACKGROUND_AGENT_QUESTIONNAIRE: Scenario = in_daemon(
    "daemon_late_background_agent_questionnaire",
    &LATE_BACKGROUND_AGENT_QUESTIONNAIRE_SCENARIO,
);

pub(super) const CANCEL_BACKGROUND_AGENT_QUESTIONNAIRE: Scenario = in_daemon(
    "daemon_cancel_background_agent_questionnaire",
    &CANCEL_BACKGROUND_AGENT_QUESTIONNAIRE_SCENARIO,
);

pub(super) const SUBAGENT_RAIL_MOUSE: Scenario =
    in_daemon("daemon_subagent_rail_mouse", &SUBAGENT_RAIL_MOUSE_SCENARIO);

pub(super) const QUIET_SUBAGENT_NOTICES: Scenario =
    in_daemon("daemon_quiet_subagent_notices", &QUIET_SUBAGENT_SCENARIO);

pub(super) const QUIET_SUBAGENT_RUNNING_NOTICES: Scenario = in_daemon(
    "daemon_quiet_subagent_running_notices",
    &RUNNING_NOTICES_SCENARIO,
);

pub(super) const STREAMING_BOUNDARY_NOTICE: Scenario = in_daemon(
    "daemon_streaming_boundary_notice",
    &STREAMING_NOTICE_SCENARIO,
);

pub(super) const BOUNDARY_NOTIFICATIONS: Scenario = in_daemon(
    "daemon_boundary_notifications",
    &super::boundary_notifications::SCENARIO,
);

pub(super) const RETRACT_STEERING_DURING_TOOL: Scenario = in_daemon(
    "daemon_retract_steering_during_tool",
    &RETRACT_STEERING_DURING_TOOL_SCENARIO,
);

pub(super) const SUBAGENT_COMPLETION_AFTER_NEW: Scenario = in_daemon(
    "daemon_subagent_completion_after_new",
    &COMPLETION_AFTER_NEW_SCENARIO,
);

pub(super) const ATTACH_PICKER: Scenario =
    in_daemon("daemon_attach_picker", &ATTACH_PICKER_SCENARIO);

pub(super) const ATTACH_PARENT_MESSAGE: Scenario = in_daemon(
    "daemon_attach_parent_message",
    &ATTACH_PARENT_MESSAGE_SCENARIO,
);

pub(super) const ATTACH_VIEW_FROM_COMMAND: Scenario = in_daemon(
    "daemon_attach_view_from_command",
    &ATTACH_VIEW_FROM_COMMAND_SCENARIO,
);

pub(super) const ATTACH_VIEW_CYCLE: Scenario =
    in_daemon("daemon_attach_view_cycle", &ATTACH_VIEW_CYCLE_SCENARIO);

pub(super) const ATTACH_VIEW_PARENT_APPROVAL: Scenario = in_daemon(
    "daemon_attach_view_parent_approval",
    &ATTACH_VIEW_PARENT_APPROVAL_SCENARIO,
);

pub(super) const ATTACH_VIEW_QUIT_RESTORES: Scenario = in_daemon(
    "daemon_attach_view_quit_restores",
    &ATTACH_VIEW_QUIT_RESTORES_SCENARIO,
);
