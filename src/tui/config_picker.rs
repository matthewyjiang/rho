use super::{PickerAction, PickerBadge, PickerBadgeTone, PickerItem, TuiInfo, UiPicker};
use crate::config::Config;
pub(super) const REASONING_VALUE: &str = "reasoning";
pub(super) const SHOW_REASONING_OUTPUT_VALUE: &str = "show_reasoning_output";
pub(super) const CHECK_FOR_UPDATES_VALUE: &str = "check_for_updates";
pub(super) const MAX_OUTPUT_BYTES_VALUE: &str = "max_output_bytes";
pub(super) const MAX_TOOL_OUTPUT_LINES_VALUE: &str = "max_tool_output_lines";
pub(super) const WEB_SEARCH_VALUE: &str = "web_search";
pub(super) const WEB_SEARCH_BACK_VALUE: &str = "web_search_back";
pub(super) const WEB_SEARCH_PROVIDER_VALUE: &str = "web_search_provider";
pub(super) const WEB_SEARCH_OPENAI_KEY_VALUE: &str = "web_search_openai_api_key";
pub(super) const WEB_SEARCH_EXA_KEY_VALUE: &str = "web_search_exa_api_key";
pub(super) const WEB_SEARCH_BRAVE_KEY_VALUE: &str = "web_search_brave_api_key";

pub(super) fn config_picker(
    info: &TuiInfo,
    max_output_bytes: usize,
    max_tool_output_lines: usize,
) -> UiPicker {
    let config = Config::load(info.config_path.clone()).unwrap_or_default();
    UiPicker::new(
        "Config",
        "type regex filter, enter change, esc cancel",
        vec![
            PickerItem {
                label: "Reasoning".into(),
                detail: Some(format!(
                    "Controls model reasoning. Current: {}; Enter cycles to {}.",
                    info.reasoning,
                    info.reasoning.next()
                )),
                preview: None,
                badge: Some(PickerBadge {
                    text: info.reasoning.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: REASONING_VALUE.into(),
            },
            PickerItem {
                label: "Show reasoning output".into(),
                detail: Some(
                    "Controls whether model reasoning text is shown in the TUI. Applies next turn."
                        .into(),
                ),
                preview: None,
                badge: Some(PickerBadge {
                    text: if info.show_reasoning_output {
                        "shown".into()
                    } else {
                        "hidden".into()
                    },
                    tone: PickerBadgeTone::Selected,
                }),
                value: SHOW_REASONING_OUTPUT_VALUE.into(),
            },
            PickerItem {
                label: "Check for updates".into(),
                detail: Some("Checks GitHub releases at startup and shows an update notice in the header when available.".into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: if config.check_for_updates {
                        "on".into()
                    } else {
                        "off".into()
                    },
                    tone: PickerBadgeTone::Selected,
                }),
                value: CHECK_FOR_UPDATES_VALUE.into(),
            },
            PickerItem {
                label: "Max output bytes".into(),
                detail: Some(
                    "Maximum tool output retained in context. Saved for next session.".into(),
                ),
                preview: None,
                badge: Some(PickerBadge {
                    text: max_output_bytes.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: MAX_OUTPUT_BYTES_VALUE.into(),
            },
            PickerItem {
                label: "Max tool output lines".into(),
                detail: Some("Maximum collapsed tool output lines shown in the TUI.".into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: max_tool_output_lines.to_string(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: MAX_TOOL_OUTPUT_LINES_VALUE.into(),
            },
            PickerItem {
                label: "Web search".into(),
                detail: Some("Configure web_search backend and API keys.".into()),
                preview: None,
                badge: Some(PickerBadge {
                    text: config.web_search_provider.clone(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: WEB_SEARCH_VALUE.into(),
            },
        ],
        PickerAction::Config,
    )
}

pub(super) fn web_search_config_picker(info: &TuiInfo) -> UiPicker {
    let config = Config::load(info.config_path.clone()).unwrap_or_default();
    UiPicker::new(
        "Web search config",
        "type regex filter, enter change, esc back",
        vec![
            PickerItem {
                label: "Back to config".into(),
                detail: Some("Return to the main config menu.".into()),
                preview: None,
                badge: None,
                value: WEB_SEARCH_BACK_VALUE.into(),
            },
            PickerItem {
                label: "Provider".into(),
                detail: Some(format!(
                    "Backend for web_search. Current: {}; Enter cycles to {}.",
                    config.web_search_provider,
                    next_web_search_provider(&config.web_search_provider)
                )),
                preview: None,
                badge: Some(PickerBadge {
                    text: config.web_search_provider.clone(),
                    tone: PickerBadgeTone::Selected,
                }),
                value: WEB_SEARCH_PROVIDER_VALUE.into(),
            },
            PickerItem {
                label: "OpenAI API key".into(),
                detail: Some("Optional key for OpenAI web search. Codex login is used automatically when available.".into()),
                preview: None,
                badge: Some(secret_badge(config.web_search_openai_api_key.as_deref())),
                value: WEB_SEARCH_OPENAI_KEY_VALUE.into(),
            },
            PickerItem {
                label: "Exa API key".into(),
                detail: Some("Optional Exa API key. Without one, Exa hosted MCP is used.".into()),
                preview: None,
                badge: Some(secret_badge(config.web_search_exa_api_key.as_deref())),
                value: WEB_SEARCH_EXA_KEY_VALUE.into(),
            },
            PickerItem {
                label: "Brave API key".into(),
                detail: Some("Optional Brave Search API key used by the brave backend.".into()),
                preview: None,
                badge: Some(secret_badge(config.web_search_brave_api_key.as_deref())),
                value: WEB_SEARCH_BRAVE_KEY_VALUE.into(),
            },
        ],
        PickerAction::Config,
    )
}

fn next_web_search_provider(current: &str) -> &'static str {
    match current {
        "auto" => "openai",
        "openai" => "exa",
        "exa" => "brave",
        "brave" => "disabled",
        "disabled" => "auto",
        _ => "auto",
    }
}

fn secret_badge(value: Option<&str>) -> PickerBadge {
    PickerBadge {
        text: if value.is_some_and(|value| !value.trim().is_empty()) {
            "set".into()
        } else {
            "unset".into()
        },
        tone: PickerBadgeTone::Selected,
    }
}
