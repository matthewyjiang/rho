//! `/mcp` inventory picker.

use crate::tools::mcp::{McpCatalog, McpServerReport, McpServerStatus, McpSessionReport};

use super::{
    picker_overlay::OverlayChrome, PickerAction, PickerBadge, PickerBadgeTone, PickerItem,
    PickerLayout, UiPicker,
};

pub(super) struct McpPickerContext<'a> {
    pub(super) report: &'a McpSessionReport,
    /// Prompts and resources are session state rather than config, so they come
    /// from the live catalog rather than the startup report.
    pub(super) catalog: &'a McpCatalog,
    pub(super) config_path: &'a std::path::Path,
}

pub(super) fn picker(context: McpPickerContext<'_>) -> UiPicker {
    let mut items = Vec::with_capacity(context.report.servers.len().saturating_add(1));
    items.push(mode_item(context.report, context.config_path));
    items.extend(
        context
            .report
            .servers
            .iter()
            .map(|server| server_item(server, context.catalog)),
    );

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
    use crate::tools::mcp::McpLoadMode;

    let config = crate::paths::display(config_path);
    let summary = report.summary();
    let (status, healthy, detail) = if !summary.configured {
        (
            "not configured".into(),
            true,
            format!("No MCP servers in {config}. Add entries under [mcp.servers]."),
        )
    } else {
        match summary.mode {
            McpLoadMode::Native if summary.connecting > 0 => (
                if summary.connecting > 0 && summary.connected == 0 {
                    "connecting".into()
                } else {
                    format!(
                        "{} connected, {} connecting",
                        summary.connected, summary.connecting
                    )
                },
                summary.problems == 0,
                format!(
                    "{} enabled, {} exported tool{}, {} problem{}. Config: {config}.",
                    summary.enabled,
                    summary.exported_tools,
                    super::plural_suffix(summary.exported_tools),
                    summary.problems,
                    super::plural_suffix(summary.problems),
                ),
            ),
            McpLoadMode::Native => (
                format!("{} connected", summary.connected),
                summary.problems == 0,
                format!(
                    "{} enabled, {} exported tool{}, {} problem{}. Config: {config}.",
                    summary.enabled,
                    summary.exported_tools,
                    super::plural_suffix(summary.exported_tools),
                    summary.problems,
                    super::plural_suffix(summary.problems),
                ),
            ),
            McpLoadMode::UnsupportedAgent => (
                "unsupported agent".into(),
                summary.enabled == 0 && summary.problems == 0,
                format!(
                    "Native MCP loads only for Rho agents. The active agent does not host MCP tools. Config: {config}."
                ),
            ),
            McpLoadMode::ToolsDisabled => (
                "tools disabled".into(),
                true,
                format!(
                    "This session started with tools disabled, so MCP was not connected. Config: {config}."
                ),
            ),
        }
    };
    PickerItem {
        section: Some("STATUS".into()),
        label: "Session".into(),
        detail: Some(detail),
        preview: None,
        badge: Some(PickerBadge {
            text: status,
            tone: if healthy {
                PickerBadgeTone::Healthy
            } else {
                PickerBadgeTone::Warning
            },
        }),
        value: "session".into(),
        selection_verb: None,
    }
}

/// What this server offers beyond tools. Named so a user can tell what to type
/// next: a prompt is a slash command, a resource is an `@` mention.
fn catalog_lines(identity: &str, catalog: &McpCatalog) -> Vec<String> {
    let prompts = catalog
        .prompts()
        .into_iter()
        .filter(|prompt| prompt.server == identity)
        .map(|prompt| format!("/{}", prompt.command_name()))
        .collect::<Vec<_>>();
    let resources = catalog
        .resources()
        .into_iter()
        .filter(|resource| resource.server == identity)
        .count();
    let mut lines = Vec::new();
    if !prompts.is_empty() {
        lines.push(format!(
            "{} prompt(s): {}",
            prompts.len(),
            prompts.join(", ")
        ));
    }
    if resources > 0 {
        lines.push(format!("{resources} resource(s), available with @"));
    }
    lines
}

fn server_item(server: &McpServerReport, catalog: &McpCatalog) -> PickerItem {
    let mut detail = server.detail_text();
    for line in catalog_lines(&server.identity, catalog) {
        detail.push('\n');
        detail.push_str(&line);
    }
    PickerItem {
        section: Some("SERVERS".into()),
        label: server.identity.clone(),
        detail: Some(detail),
        preview: None,
        badge: Some(PickerBadge {
            text: server.status().as_str().into(),
            tone: if server.status().is_healthy() {
                if server.status() == McpServerStatus::Connected {
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
