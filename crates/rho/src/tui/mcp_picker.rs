//! `/mcp` inventory picker.

use crate::tools::mcp::{McpServerReport, McpServerStatus, McpSessionReport};

use super::{
    picker_overlay::OverlayChrome, PickerAction, PickerBadge, PickerBadgeTone, PickerItem,
    PickerLayout, UiPicker,
};

pub(super) struct McpPickerContext<'a> {
    pub(super) report: &'a McpSessionReport,
    pub(super) config_path: &'a std::path::Path,
}

pub(super) fn picker(context: McpPickerContext<'_>) -> UiPicker {
    let mut items = Vec::with_capacity(context.report.servers.len().saturating_add(1));
    items.push(mode_item(context.report, context.config_path));
    items.extend(context.report.servers.iter().map(server_item));

    UiPicker::new("MCP servers", items, PickerAction::Dismiss)
        .with_layout(PickerLayout::Overlay)
        .with_badge_placement(super::PickerBadgePlacement::Detail)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " SERVERS".into(),
            detail_label: Some(" DETAILS".into()),
            nav_keys_hint: "↑↓ servers".into(),
        })
        .with_confirm_verb("close")
}

fn mode_item(report: &McpSessionReport, config_path: &std::path::Path) -> PickerItem {
    let presentation = report.picker_session_presentation(config_path);
    PickerItem {
        section: Some("STATUS".into()),
        label: "Session".into(),
        detail: Some(presentation.detail),
        preview: None,
        badge: Some(PickerBadge {
            text: presentation.status,
            tone: if presentation.healthy {
                PickerBadgeTone::Healthy
            } else {
                PickerBadgeTone::Warning
            },
        }),
        value: "session".into(),
        selection_verb: None,
    }
}

fn server_item(server: &McpServerReport) -> PickerItem {
    PickerItem {
        section: Some("SERVERS".into()),
        label: server.identity.clone(),
        detail: Some(server.detail_text()),
        preview: None,
        badge: Some(PickerBadge {
            text: server.status.as_str().into(),
            tone: if server.status.is_healthy() {
                if server.status == McpServerStatus::Connected {
                    PickerBadgeTone::Healthy
                } else {
                    PickerBadgeTone::Internal
                }
            } else {
                PickerBadgeTone::Warning
            },
        }),
        value: server.identity.clone(),
        selection_verb: None,
    }
}
