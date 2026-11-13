//! `/mcp` inventory picker for the daemon session's MCP inventory.

use rho_protocol::{SessionMcpInventory, SessionMcpMode, SessionMcpServer, SessionMcpStatus};

use super::{
    picker::OverlayChrome, PickerBadge, PickerBadgeTone, PickerItem, PickerLayout, UiPicker,
};

pub(super) fn picker(inventory: &SessionMcpInventory) -> UiPicker {
    let mut items = Vec::with_capacity(inventory.servers.len().saturating_add(1));
    items.push(mode_item(inventory));
    items.extend(inventory.servers.iter().map(server_item));
    inventory_picker(items)
}

pub(super) fn loading() -> UiPicker {
    inventory_picker(vec![PickerItem::new(
        "Loading session inventory…".into(),
        "loading".into(),
    )])
}

fn inventory_picker(items: Vec<PickerItem>) -> UiPicker {
    UiPicker::view_mcp("MCP servers", items)
        .with_layout(PickerLayout::Overlay)
        .with_badge_placement(super::PickerBadgePlacement::Detail)
        .with_overlay_chrome(OverlayChrome {
            nav_label: " SERVERS".into(),
            detail_label: Some(" DETAILS".into()),
            nav_keys_hint: "↑↓ servers".into(),
        })
        .with_confirm_verb("close")
}

fn mode_item(inventory: &SessionMcpInventory) -> PickerItem {
    let config = crate::paths::display(&inventory.config_path);
    let connected = inventory
        .servers
        .iter()
        .filter(|server| server.status == SessionMcpStatus::Connected)
        .count();
    let connecting = inventory
        .servers
        .iter()
        .filter(|server| server.status == SessionMcpStatus::Connecting)
        .count();
    let problems = inventory
        .servers
        .iter()
        .filter(|server| {
            !matches!(
                server.status,
                SessionMcpStatus::Connected
                    | SessionMcpStatus::Connecting
                    | SessionMcpStatus::Disabled
                    | SessionMcpStatus::NotLoaded
            )
        })
        .count();
    let enabled = inventory
        .servers
        .iter()
        .filter(|server| server.enabled)
        .count();
    let exported_tools = inventory.servers.iter().fold(0_u64, |total, server| {
        total.saturating_add(server.tool_count)
    });
    let (status, healthy, detail) = if inventory.servers.is_empty() {
        (
            "not configured".into(),
            true,
            format!("No MCP servers in {config}. Add entries under [mcp.servers]."),
        )
    } else {
        match inventory.mode {
            SessionMcpMode::Native => (
                if connecting == 0 { format!("{connected} connected") }
                else if connected == 0 { "connecting".into() }
                else { format!("{connected} connected, {connecting} connecting") },
                problems == 0,
                format!(
                    "{enabled} enabled, {exported_tools} exported tool{}, {problems} problem{}. Config: {config}.",
                    if exported_tools == 1 { "" } else { "s" },
                    super::plural_suffix(problems),
                ),
            ),
            SessionMcpMode::UnsupportedAgent => (
                "unsupported agent".into(),
                enabled == 0 && problems == 0,
                format!("Native MCP loads only for Rho agents. The active agent does not host MCP tools. Config: {config}."),
            ),
            SessionMcpMode::ToolsDisabled => (
                "tools disabled".into(),
                true,
                format!("This session started with tools disabled, so MCP was not connected. Config: {config}."),
            ),
            _ => ("unknown".into(), false, format!("Unknown MCP load mode. Config: {config}.")),
        }
    };
    PickerItem {
        section: Some("STATUS".into()),
        detail: Some(detail.into()),
        badge: Some(PickerBadge {
            text: status,
            tone: if healthy {
                PickerBadgeTone::Healthy
            } else {
                PickerBadgeTone::Warning
            },
        }),
        ..PickerItem::new("Session".into(), "session".into())
    }
}

fn server_item(server: &SessionMcpServer) -> PickerItem {
    let (status, tone) = match server.status {
        SessionMcpStatus::Connected => ("connected", PickerBadgeTone::Healthy),
        SessionMcpStatus::Connecting => ("connecting", PickerBadgeTone::Internal),
        SessionMcpStatus::Disabled => ("disabled", PickerBadgeTone::Internal),
        SessionMcpStatus::NotLoaded => ("not loaded", PickerBadgeTone::Internal),
        SessionMcpStatus::InvalidConfig => ("invalid", PickerBadgeTone::Warning),
        SessionMcpStatus::Failed => ("failed", PickerBadgeTone::Warning),
        SessionMcpStatus::TimedOut => ("timeout", PickerBadgeTone::Warning),
        _ => ("unknown", PickerBadgeTone::Warning),
    };
    PickerItem {
        section: Some("SERVERS".into()),
        detail: Some(server.detail.clone().into()),
        badge: Some(PickerBadge {
            text: status.into(),
            tone,
        }),
        ..PickerItem::new(server.identity.clone(), server.identity.clone())
    }
}
