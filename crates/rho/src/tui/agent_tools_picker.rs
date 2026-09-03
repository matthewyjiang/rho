//! Multi-select tool picker for the agent editor.
//!
//! Each runtime has its own vocabulary: Rho capabilities (plus `all`), the
//! offered Claude Code tool names (plus an `Other…` escape to free text for
//! specifiers and MCP names), and the closed Cursor allow list. Rows carry an
//! `on` badge when the draft allows them; confirming a row toggles it and the
//! picker stays open.

use super::{
    agent_editor::{AGENT_TOOL_ALL, AGENT_TOOL_OTHER, AGENT_TOOL_ROW_PREFIX},
    PickerBadge, PickerBadgeTone, PickerItem, UiPicker,
};
use crate::agent::{
    AgentDefinition, AgentRuntimeSpec, CursorTool, ToolPolicy, BUILTIN_TOOL_CAPABILITIES,
};
use crate::claude_runtime::tools as claude_tools;

/// Builds the tools picker for `draft`'s current runtime.
pub(super) fn agent_tools_picker(draft: &AgentDefinition) -> UiPicker {
    let items = match &draft.runtime {
        AgentRuntimeSpec::Rho { tools, .. } => rho_items(tools),
        AgentRuntimeSpec::ClaudeCli(config) => claude_items(config.tools.as_slice()),
        AgentRuntimeSpec::Cursor(config) => cursor_items(&config.tools),
    };
    UiPicker::edit_agent("tools", items)
        .with_confirm_verb("toggle")
        .with_space_confirm()
}

fn on_badge() -> PickerBadge {
    PickerBadge {
        text: "on".into(),
        tone: PickerBadgeTone::Selected,
    }
}

fn tool_row(name: &str, detail: impl Into<String>, on: bool, value: String) -> PickerItem {
    PickerItem {
        section: None,
        label: name.into(),
        detail: Some(detail.into()),
        preview: None,
        badge: on.then(on_badge),
        value,
        selection_verb: None,
        allow_filter_completion: true,
    }
}

fn rho_items(tools: &ToolPolicy) -> Vec<PickerItem> {
    let all = matches!(tools, ToolPolicy::All);
    let mut items = vec![PickerItem {
        section: None,
        label: "all".into(),
        detail: Some("Every host tool, including ones added later.".into()),
        preview: None,
        badge: all.then(on_badge),
        value: AGENT_TOOL_ALL.into(),
        selection_verb: Some("select"),
        allow_filter_completion: true,
    }];
    items.extend(BUILTIN_TOOL_CAPABILITIES.iter().map(|capability| {
        let on = match tools {
            ToolPolicy::All => true,
            ToolPolicy::Allow(set) => set.contains(capability),
        };
        tool_row(
            capability.as_str(),
            capability.detail(),
            on,
            format!("{AGENT_TOOL_ROW_PREFIX}{capability}"),
        )
    }));
    items
}

fn claude_items(current: &[String]) -> Vec<PickerItem> {
    let mut items: Vec<PickerItem> = claude_tools::CLAUDE_TOOLS
        .iter()
        .map(|tool| {
            tool_row(
                tool.name,
                tool.detail,
                current.iter().any(|name| name == tool.name),
                format!("{AGENT_TOOL_ROW_PREFIX}{}", tool.name),
            )
        })
        .collect();
    // Keep configured names Rho does not offer (specifiers, MCP tools) so they
    // stay visible and can be switched off without retyping the list.
    items.extend(
        current
            .iter()
            .filter(|name| !claude_tools::is_offered_tool(name))
            .map(|name| {
                tool_row(
                    name,
                    "Set in the agent file. Passed through on --tools unchanged.",
                    true,
                    format!("{AGENT_TOOL_ROW_PREFIX}{name}"),
                )
            }),
    );
    items.push(PickerItem {
        section: None,
        label: "Other…".into(),
        detail: Some(
            "Edit the list as text for specifiers such as Bash(git *) or MCP tool names.".into(),
        ),
        preview: None,
        badge: None,
        value: AGENT_TOOL_OTHER.into(),
        selection_verb: Some("edit"),
        allow_filter_completion: false,
    });
    items
}

fn cursor_items(current: &[CursorTool]) -> Vec<PickerItem> {
    CursorTool::ALL
        .iter()
        .map(|tool| {
            tool_row(
                tool.label(),
                format!("{} ({})", tool.detail(), tool.capability_kind().label()),
                current.contains(tool),
                format!("{AGENT_TOOL_ROW_PREFIX}{}", tool.as_flag()),
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "agent_tools_picker_tests.rs"]
mod tests;
